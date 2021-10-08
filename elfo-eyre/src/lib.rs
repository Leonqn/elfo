#![warn(rust_2018_idioms, unreachable_pub)]

use std::{error::Error, panic::Location};

use backtrace::Backtrace;
use eyre::EyreHandler;

pub use eyre::{bail, ensure, eyre, Report, Result, WrapErr};

pub struct Handler {
    backtrace: Option<Backtrace>,
    location: Option<&'static Location<'static>>,
}

impl EyreHandler for Handler {
    #[inline]
    fn debug(
        &self,
        error: &(dyn Error + 'static),
        f: &mut core::fmt::Formatter<'_>,
    ) -> core::fmt::Result {
        // TODO: use the default one?
        core::fmt::Debug::fmt(error, f)
    }

    #[inline]
    fn track_caller(&mut self, location: &'static Location<'static>) {
        self.location = Some(location);
    }
}

impl Handler {
    #[inline]
    pub fn location(&self) -> Option<&'static Location<'static>> {
        self.location
    }

    #[inline]
    pub fn backtrace_vec(&mut self) -> Vec<String> {
        let backtrace = match self.backtrace.as_mut() {
            Some(backtrace) => backtrace,
            None => return Vec::new(),
        };

        backtrace.resolve();
        vec![String::from("1"), String::from("2")]
    }
}

struct Hook {
    capture_backtrace: bool,
}

impl Hook {
    fn make_handler(&self, _error: &(dyn Error + 'static)) -> Handler {
        let backtrace = if self.capture_backtrace {
            Some(Backtrace::new_unresolved())
        } else {
            None
        };

        Handler {
            backtrace,
            location: None,
        }
    }
}

// TODO: or `init`?
pub fn install() -> Result<(), impl Error> {
    // TODO: RUST_LIB_BACKTRACE
    let capture_backtrace = std::env::var("RUST_BACKTRACE").map_or(false, |val| val != "0");
    let hook = Hook { capture_backtrace };

    eyre::set_hook(Box::new(move |e| Box::new(hook.make_handler(e))))
}
