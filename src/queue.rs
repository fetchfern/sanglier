use std::time::Duration;
use std::sync::Arc;
use std::borrow::Cow;

use tokio::sync::{mpsc, Semaphore};
use tokio::time::sleep;
use smallvec::SmallVec;
use serde::Serialize;
use reqwest::header::{CONTENT_TYPE, HeaderValue};

const MIME_JSON: HeaderValue = HeaderValue::from_static("application/json");

use crate::event::{NewEvent, Event};

pub struct PostHogBuilder {
    tick: Duration, 
    api_key: String,
    base_url: String,
    user_agent: String,
}

impl Default for PostHogBuilder {
    fn default() -> Self {
        Self {
            tick: Duration::from_secs(2),
            api_key: String::new(),
            base_url: String::new(),
            user_agent: "sanglier/0.1.0".to_owned(),
        }
    }
}

impl PostHogBuilder {
    pub fn batch_delay(mut self, tick: Duration) -> Self {
        self.tick = tick;
        self
    }

    pub fn in_official_us_region(mut self) -> Self {
        self.base_url = "https://us.i.posthog.com".to_owned();
        self
    }

    pub fn in_official_eu_region(mut self) -> Self {
        self.base_url = "https://eu.i.posthog.com".to_owned();
        self
    }

    pub fn with_base_url(mut self, url: impl AsRef<str>) -> Self {
        self.base_url = url.as_ref().trim_end_matches('/').to_owned();
        self
    }

    pub fn with_api_key(mut self, api_key: impl Into<String>) -> Self {
        self.api_key = api_key.into();
        self
    }

    pub fn with_user_agent(mut self, user_agent: impl Into<String>) -> Self {
        self.user_agent = user_agent.into();
        self
    }

    pub fn drive<P: Serialize + Send + Sync + 'static>(self) -> reqwest::Result<PostHog<P>> {
        if self.api_key.is_empty() {
            panic!("Missing PostHog API key");
        }

        if self.base_url.is_empty() {
            panic!("Missing PostHog base URL");
        }

        let (send, recv) = mpsc::unbounded_channel();

        let http = reqwest::ClientBuilder::default()
            .user_agent(&self.user_agent)
            .build()?;

        let wakeup = Arc::new(Semaphore::new(0));

        let ph = PostHog {
            send: Arc::new(send),
            wakeup: Arc::clone(&wakeup),
        };

        tokio::spawn(runtime(self, http, recv, wakeup));

        Ok(ph)
    }
}

#[derive(Clone)]
pub struct PostHog<P> {
    send: Arc<mpsc::UnboundedSender<Event<Option<P>>>>,
    wakeup: Arc<Semaphore>,
}

impl<P> PostHog<P> {
    pub fn capture(&self, name: &'static str) -> NewEvent<P> {
        NewEvent {
            event: Event {
                name: Cow::Borrowed(name),
                distinct_id: String::new(),
                properties: None,
                #[cfg(feature = "precise_timings")]
                timestamp: chrono::Utc::now(),
            },
            hog: self,
        }
    }

    pub fn force_process(&self) {
        if self.wakeup.available_permits() == 0 {
            self.wakeup.add_permits(1)
        }
    }

    pub(crate) fn send(&self, event: Event<Option<P>>) {
        let _ = self.send.send(event);
    }
}

async fn runtime<P: Serialize + Send + Sync + 'static>(
    hog: PostHogBuilder,
    http: reqwest::Client,
    mut recv: mpsc::UnboundedReceiver<Event<Option<P>>>,
    wakeup: Arc<Semaphore>,
) {
    #[derive(Serialize)]
    struct Batch<'a, P: Serialize> {
        api_key: &'a str,
        batch: &'a SmallVec<[EventRecord<P>; 32]>,
    }

    #[derive(Serialize)]
    struct EventProperties<P: Serialize> {
        #[serde(rename = "$process_person_profile", skip_serializing_if = "Clone::clone")]
        process_person_profile: bool,
        #[serde(rename = "$lib_name")]
        lib_name: &'static str,
        #[serde(flatten, skip_serializing_if = "Option::is_none")]
        properties: Option<P>,
    }

    #[derive(Serialize)]
    struct EventRecord<P: Serialize> {
        event: Cow<'static, str>,
        #[serde(skip_serializing_if = "str::is_empty")]
        distinct_id: String,
        properties: EventProperties<P>,
        #[cfg(feature = "precise_timings")]
        timestamp: String,
    }

    let batch_endpoint = format!("{}/batch", hog.base_url);
    let mut events_serde: SmallVec<[EventRecord<P>; 32]> = smallvec::smallvec![];
    let mut events_chan = Vec::with_capacity(32);

    const PROCESS_MAX: usize = 128;

    loop {
        if recv.is_empty() {
            let wait = sleep(hog.tick);

            tokio::select! {
                _ = wait => {}
                r = wakeup.acquire() => {
                    if let Ok(permit) = r {
                        permit.forget();
                    }
                }
            }
        }

        if let Ok(permit) = wakeup.try_acquire() {
            permit.forget();
        }

        let max = recv.len().min(PROCESS_MAX);
        if max == 0 {
            continue;
        }

        // shouldn't sleep
        let processed = recv.recv_many(&mut events_chan, max).await;

        for ev in events_chan.drain(..) {
            events_serde.push(EventRecord {
                event: ev.name,
                properties: EventProperties {
                    process_person_profile: !ev.distinct_id.is_empty(),
                    lib_name: "sanglier",
                    properties: ev.properties,
                },
                distinct_id: ev.distinct_id,
                #[cfg(feature = "precise_timings")]
                timestamp: ev.timestamp.format("%+").to_string(),
            });
        }

        let batch = Batch {
            api_key: &hog.api_key,
            batch: &events_serde,
        };

        match serde_json::to_string(&batch) {
            Ok(serialized) => {
                #[cfg(feature = "debug_logs")]
                println!("sending the following body: {serialized}");

                let result = http.post(&batch_endpoint)
                    .header(CONTENT_TYPE, MIME_JSON)
                    .body(serialized)
                    .send()
                    .await
                    .and_then(|resp| resp.error_for_status());

                #[cfg(feature = "debug_logs")]
                match result {
                    Ok(resp) => {
                        let response = resp.text().await.unwrap();
                        println!("responded with {response}");
                    }

                    Err(e) => {
                        eprintln!("[sanglier/posthog] failed to send batch request: {e}");
                    }
                }

                #[cfg(not(feature = "debug_logs"))]
                if let Err(e) = result {
                    eprintln!("[sanglier/posthog] failed to send batch request: {e}");
                }
            }

            Err(e) => {
                eprintln!("[sanglier/posthog] failed to serialize batch events to string: {e}");
            }
        }

        events_serde.clear();

        if processed < max {
            break;
        }
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;
    use std::env::var;

    use serde::Serialize;
    use tokio::time::sleep;

    use super::*;

    #[test]
    fn test_capture() {
        dotenvy::dotenv().unwrap();

        let rt = tokio::runtime::Runtime::new().unwrap();
        let _guard = rt.enter();

        #[derive(Serialize)]
        #[serde(untagged)]
        enum Properties {
            SignUp {
                wants_promo_emails: bool,
            },
        }

        let hog = PostHogBuilder::default()
            .with_base_url(var("POSTHOG_BASE_URL").unwrap())
            .with_api_key(var("POSTHOG_API_KEY").unwrap())
            .drive::<Properties>()
            .unwrap();

        hog.capture("user_sign_up")
            .identify("1234xxxx")
            .properties(Properties::SignUp {
                wants_promo_emails: false,
            })
            .enqueue();

        hog.force_process();
        rt.block_on(async move {
            sleep(Duration::from_secs(5)).await;
        });
    }
}
