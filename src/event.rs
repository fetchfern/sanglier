use std::borrow::Cow;

#[cfg(feature = "precise_timings")]
use chrono::{DateTime, Utc};

use crate::queue::PostHog;

pub struct Event<P> {
    pub(crate) name: Cow<'static, str>,
    pub(crate) distinct_id: String,
    pub(crate) properties: P,
    #[cfg(feature = "precise_timings")]
    pub(crate) timestamp: DateTime<Utc>,
}

#[must_use]
pub struct NewEvent<'a, P> {
    pub(crate) event: Event<Option<P>>,
    pub(crate) hog: &'a PostHog<P>,
}

impl<'a, P> NewEvent<'a, P> {
    pub fn identify(mut self, distinct_id: impl Into<String>) -> NewEventWithIdentity<'a, P> {
        self.event.distinct_id = distinct_id.into();
        NewEventWithIdentity(self)
    }

    pub fn anonymous(self) -> NewEventWithIdentity<'a, P> {
        NewEventWithIdentity(self)
    }
}

#[must_use]
#[repr(transparent)]
pub struct NewEventWithIdentity<'a, P>(NewEvent<'a, P>);

impl<P> NewEventWithIdentity<'_, P> {
    pub fn properties(mut self, properties: P) -> Self {
        self.0.event.properties = Some(properties);
        self
    }

    pub fn enqueue(self) {
        let NewEvent { hog, event } = self.0;
        hog.send(event);
    }
}
