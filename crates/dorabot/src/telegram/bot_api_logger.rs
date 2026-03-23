use std::future::IntoFuture;

use teloxide::prelude::Bot as TeloxideBot;
use teloxide::requests::{HasPayload, Output, Payload, Request, Requester};
use teloxide::types::*;
use url::Url;

#[derive(Clone, Debug)]
pub struct Bot {
    inner: TeloxideBot,
}

impl Bot {
    pub fn new(inner: TeloxideBot) -> Self {
        Self { inner }
    }
}

impl std::ops::Deref for Bot {
    type Target = TeloxideBot;

    fn deref(&self) -> &Self::Target {
        &self.inner
    }
}

#[must_use = "Requests are lazy and do nothing unless sent"]
#[derive(Clone)]
pub struct LoggedRequest<R> {
    inner: R,
    api_url: String,
    token: String,
}

impl<R> LoggedRequest<R> {
    fn log_url(&self)
    where
        R: HasPayload,
        R::Payload: Payload,
    {
        let method = R::Payload::NAME.trim_end_matches("Inline");

        // Skip logging for GetUpdates - too noisy
        if method == "GetUpdates" {
            return;
        }

        let base = self.api_url.trim_end_matches('/');
        let masked = self.token.split(':').next().unwrap_or("?");
        log::debug!("Bot API → {}/bot{}:***/{}", base, masked, method);
    }
}

impl<R> HasPayload for LoggedRequest<R>
where
    R: HasPayload,
{
    type Payload = R::Payload;

    fn payload_mut(&mut self) -> &mut Self::Payload {
        self.inner.payload_mut()
    }

    fn payload_ref(&self) -> &Self::Payload {
        self.inner.payload_ref()
    }
}

impl<R> Request for LoggedRequest<R>
where
    R: Request + HasPayload,
    R::Payload: Payload,
{
    type Err = R::Err;

    type Send = R::Send;

    type SendRef = R::SendRef;

    fn send(self) -> Self::Send {
        self.log_url();
        self.inner.send()
    }

    fn send_ref(&self) -> Self::SendRef {
        self.log_url();
        self.inner.send_ref()
    }
}

impl<R> IntoFuture for LoggedRequest<R>
where
    R: Request + HasPayload,
    R::Payload: Payload,
{
    type Output = Result<Output<Self>, <Self as Request>::Err>;
    type IntoFuture = <Self as Request>::Send;

    fn into_future(self) -> Self::IntoFuture {
        self.send()
    }
}

macro_rules! fty {
    ($T:ident) => {
        LoggedRequest<<TeloxideBot as Requester>::$T>
    };
}

macro_rules! fwd_inner {
    ($m:ident $this:ident ($($arg:ident : $T:ty),*)) => {
        LoggedRequest {
            inner: $this.inner.$m($($arg),*),
            api_url: $this.inner.api_url().to_string(),
            token: $this.inner.token().to_string(),
        }
    };
}

// requester_forward! macro + impl Requester for Bot — 1,558 lines of generated code
include!("bot_api_logger_methods.rs");
