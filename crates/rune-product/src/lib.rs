//! A Rune app's identity per build channel: its name, id, URL scheme and folder name.
//!
//! One app ships as builds of several channels: stable (the app itself), beta, and dev (a run
//! from the terminal, or any build that doesn't say). Each non-stable build has its own id,
//! URL scheme and folder (`com.example.app.dev`, `example-dev`), so it never reads a release's
//! settings, takes its links or registers under its bundle id, and both can run side by side.
//! The channel comes from the build ([`Channel::from_build`] with the app's own
//! `option_env!("<APP>_CHANNEL")`), never from where the binary happens to sit.

/// Which build of the app this is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Channel {
    /// The released app.
    Stable,
    /// A preview, installed beside the released app.
    Beta,
    /// A development or local build.
    Dev,
}

impl Channel {
    /// The channel a build variable names: `stable` or `beta`; anything else, or nothing, is
    /// [`Channel::Dev`], so a build only takes the release's identity when it asks for it.
    pub fn from_build(value: Option<&str>) -> Channel {
        match value {
            Some("stable") => Channel::Stable,
            Some("beta") => Channel::Beta,
            _ => Channel::Dev,
        }
    }

    /// The name a build variable gives it.
    pub fn as_str(self) -> &'static str {
        match self {
            Channel::Stable => "stable",
            Channel::Beta => "beta",
            Channel::Dev => "dev",
        }
    }

    /// What a build of this channel adds to the app's id and scheme, and its name's suffix.
    fn suffix(self) -> Option<(&'static str, &'static str)> {
        match self {
            Channel::Stable => None,
            Channel::Beta => Some(("beta", "Beta")),
            Channel::Dev => Some(("dev", "Dev")),
        }
    }
}

/// One build's identity.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Product {
    name: String,
    id: String,
    scheme: String,
    family: String,
    channel: Channel,
}

impl Product {
    /// The app's build of `channel`, from the released app's name, reverse-DNS id
    /// (`com.example.app`) and URL scheme (`example`). Fails on an empty name, an id that isn't
    /// reverse-DNS (it names folders, so no `/`, spaces or empty labels) or a scheme that isn't
    /// lowercase URL scheme text.
    pub fn new(name: &str, id: &str, scheme: &str, channel: Channel) -> Result<Product, String> {
        if name.trim().is_empty() {
            return Err("an app's name can't be empty".into());
        }
        if !is_reverse_dns(id) {
            return Err(format!("{id:?} isn't a reverse-DNS id like com.example.app"));
        }
        if !is_scheme(scheme) {
            return Err(format!("{scheme:?} isn't a lowercase URL scheme like example"));
        }
        let (name, own_id, own_scheme) = match channel.suffix() {
            None => (name.to_owned(), id.to_owned(), scheme.to_owned()),
            Some((tag, title)) => (format!("{name} {title}"), format!("{id}.{tag}"), format!("{scheme}-{tag}")),
        };
        Ok(Product { name, id: own_id, scheme: own_scheme, family: id.to_owned(), channel })
    }

    /// What people see: `Example`, `Example Beta`, `Example Dev`.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// This build's id (its bundle id): `com.example.app`, `….beta`, `….dev`.
    pub fn id(&self) -> &str {
        &self.id
    }

    /// This build's URL scheme: `example`, `example-beta`, `example-dev`.
    pub fn scheme(&self) -> &str {
        &self.scheme
    }

    /// The folder name this build keeps its files under (`~/Library/Application Support/<it>`):
    /// its id, so channels never share settings or data.
    pub fn folder(&self) -> &str {
        &self.id
    }

    /// The id every channel shares: the released app's. For what one user keeps once for all of
    /// the app's builds (a Keychain service, say).
    pub fn family(&self) -> &str {
        &self.family
    }

    pub fn channel(&self) -> Channel {
        self.channel
    }
}

fn is_reverse_dns(id: &str) -> bool {
    let label = |l: &str| !l.is_empty() && l.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_');
    id.split('.').count() >= 2 && id.split('.').all(label)
}

/// RFC 3986's scheme, lowercase only: macOS matches schemes without case, so one spelling.
fn is_scheme(scheme: &str) -> bool {
    let mut chars = scheme.chars();
    chars.next().is_some_and(|c| c.is_ascii_lowercase())
        && chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, '+' | '-' | '.'))
}

#[cfg(test)]
mod tests;
