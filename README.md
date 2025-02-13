# sanglier

An async-first, strongly typed PostHog API SDK.

### Quickstart

```sh
cargo add sanglier
```

### Example

```rust
use sanglier::PostHogBuilder;
use serde::Serialize;

use std::env::var;
use std::time::Duration;

#[derive(Serialize)]
#[serde(untagged)] // important!
pub enum CustomProperties {
    SignUp {
        wants_promo_emails: bool,
    },
    Login {
        password_retries: u32,
        underwent_manual_captcha: bool,
    },
    LoginFailure {
        email: String,
    }
}

#[tokio::main]
async fn main() {
    // Sets up a background task which will batch-send events
    // every 2 seconds.
    let hog: PostHog = PostHogBuilder::default()
        .in_official_us_region()
        .with_api_key(var("POSTHOG_API_KEY").unwrap())
        .batch_delay(Duration::from_secs(2))
        .drive::<CustomProperties>()
        .unwrap();

    // `PostHog` can be cheaply cloned; it's all reference-counted!
    let hog_clone = hog.clone();

    tokio::spawn(async move {
        hog_clone.capture("user_sign_up")
            .identify("8fa3c55")
            .properties(CustomProperties::SignUp 
                wants_promo_emails: true,
            })
            .enqueue();
    });

    tokio::spawn(async move {
        // The compiler won't let you not call `identify` or `anonymous`!
        hog.capture("user_login")
            .anonymous()
            .properties(CustomProperties::LoginFailure {
                email: "example@temp.mail".to_owned(),
            })
            .enqueue();
    });

    // In ~2 seconds, events will be sent to PostHog
}
```

### Disclaimers

- **This project is not production-ready**.
- This project is not official.
- This project has not been endorsed by PostHog.

