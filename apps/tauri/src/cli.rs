//! Command line: which daemon the window shows.

use std::path::PathBuf;

use {anyhow::Context, secrecy::SecretString, url::Url};

use crate::bench::BenchPage;

/// Environment variable holding the remote daemon's API key. Never a CLI
/// argument: arguments are visible to every user through `ps`.
pub const API_KEY_ENV: &str = "ANTRE_API_KEY";

#[derive(Debug)]
pub enum Mode {
    /// Attach to the daemon configured on this machine when it answers,
    /// otherwise start one as a supervised child process.
    Auto,
    /// Always start a supervised `antre` child process.
    Sidecar(SidecarOptions),
    /// Run the gateway inside this process (`embedded` feature).
    Embedded { port: Option<u16> },
    /// A daemon on another machine.
    Remote {
        url: Url,
        api_key: Option<SecretString>,
    },
    /// Load one of the bundled benchmark pages and write its report.
    Bench {
        page: BenchPage,
        out: Option<PathBuf>,
    },
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct SidecarOptions {
    /// The `antre` binary; found next to the shell, then on `PATH`.
    pub binary: Option<PathBuf>,
    /// Use a throw-away config and data directory instead of `~/.antre`, so
    /// the sidecar never fights a running user service over its database.
    pub isolated: bool,
}

const USAGE: &str = "usage: antre-shell [--sidecar [--antre PATH] [--isolated]] [--embedded \
                     [--port N]] [--remote URL] [--bench stream|transcript|canvas [--out FILE]]";

/// Parse the arguments after the program name. `env` reads environment
/// variables (injected for tests).
pub fn parse(
    args: impl IntoIterator<Item = String>,
    env: impl Fn(&str) -> Option<String>,
) -> anyhow::Result<Mode> {
    let mut mode = Mode::Auto;
    let mut sidecar = SidecarOptions::default();
    let mut port = None;
    let mut out = None;
    let mut args = args.into_iter();
    while let Some(arg) = args.next() {
        let mut value = |name: &str| {
            args.next()
                .with_context(|| format!("{name} needs a value\n{USAGE}"))
        };
        match arg.as_str() {
            "--sidecar" => mode = Mode::Sidecar(SidecarOptions::default()),
            "--antre" => sidecar.binary = Some(PathBuf::from(value("--antre")?)),
            "--isolated" => sidecar.isolated = true,
            "--embedded" => mode = Mode::Embedded { port: None },
            "--port" => {
                port = Some(
                    value("--port")?
                        .parse::<u16>()
                        .context("--port must be a TCP port")?,
                );
            },
            "--remote" => {
                let url = parse_remote_url(&value("--remote")?)?;
                let api_key = env(API_KEY_ENV)
                    .filter(|key| !key.trim().is_empty())
                    .map(SecretString::new);
                mode = Mode::Remote { url, api_key };
            },
            "--bench" => {
                let page = value("--bench")?.parse::<BenchPage>()?;
                mode = Mode::Bench { page, out: None };
            },
            "--out" => out = Some(PathBuf::from(value("--out")?)),
            "-h" | "--help" => anyhow::bail!("{USAGE}"),
            other => anyhow::bail!("unknown argument `{other}`\n{USAGE}"),
        }
    }
    Ok(match mode {
        Mode::Sidecar(_) => Mode::Sidecar(sidecar),
        Mode::Auto if sidecar != SidecarOptions::default() => Mode::Sidecar(sidecar),
        Mode::Embedded { .. } => Mode::Embedded { port },
        Mode::Bench { page, .. } => Mode::Bench { page, out },
        other => other,
    })
}

/// Only `https`, or `http` to a loopback address: a remote daemon's session
/// cookie and API key must not cross the network in clear text.
fn parse_remote_url(input: &str) -> anyhow::Result<Url> {
    let url = Url::parse(input.trim()).with_context(|| format!("invalid URL `{input}`"))?;
    match url.scheme() {
        "https" => Ok(url),
        "http" if is_loopback(&url) => Ok(url),
        "http" => anyhow::bail!("refusing plain http to a non-loopback host; use https"),
        other => anyhow::bail!("unsupported URL scheme `{other}`"),
    }
}

fn is_loopback(url: &Url) -> bool {
    match url.host() {
        Some(url::Host::Domain(domain)) => domain.eq_ignore_ascii_case("localhost"),
        Some(url::Host::Ipv4(ip)) => ip.is_loopback(),
        Some(url::Host::Ipv6(ip)) => ip.is_loopback(),
        None => false,
    }
}

#[allow(clippy::unwrap_used, clippy::expect_used)]
#[cfg(test)]
mod tests {
    use secrecy::ExposeSecret;

    use super::*;

    fn args(list: &[&str]) -> Vec<String> {
        list.iter().map(|arg| (*arg).to_owned()).collect()
    }

    fn no_env(_: &str) -> Option<String> {
        None
    }

    #[test]
    fn defaults_to_auto() {
        assert!(matches!(parse(args(&[]), no_env).unwrap(), Mode::Auto));
    }

    #[test]
    fn sidecar_options_apply_in_any_order() {
        let mode = parse(
            args(&["--isolated", "--sidecar", "--antre", "/opt/antre"]),
            no_env,
        )
        .unwrap();
        let Mode::Sidecar(options) = mode else {
            panic!("expected sidecar, got {mode:?}");
        };
        assert!(options.isolated);
        assert_eq!(options.binary, Some(PathBuf::from("/opt/antre")));
    }

    #[test]
    fn sidecar_flags_alone_imply_sidecar() {
        assert!(matches!(
            parse(args(&["--isolated"]), no_env).unwrap(),
            Mode::Sidecar(SidecarOptions { isolated: true, .. })
        ));
    }

    #[test]
    fn remote_reads_the_key_from_the_environment() {
        let env = |name: &str| (name == API_KEY_ENV).then(|| "mk_secret".to_owned());
        let Mode::Remote { url, api_key } =
            parse(args(&["--remote", "https://box.example:8443"]), env).unwrap()
        else {
            panic!("expected remote");
        };
        assert_eq!(url.as_str(), "https://box.example:8443/");
        assert_eq!(api_key.unwrap().expose_secret(), "mk_secret");
    }

    #[test]
    fn remote_ignores_a_blank_key() {
        let env = |_: &str| Some("  ".to_owned());
        let Mode::Remote { api_key, .. } =
            parse(args(&["--remote", "https://box.example"]), env).unwrap()
        else {
            panic!("expected remote");
        };
        assert!(api_key.is_none());
    }

    #[test]
    fn remote_refuses_clear_text_to_other_hosts() {
        assert!(parse(args(&["--remote", "http://box.example"]), no_env).is_err());
        assert!(parse(args(&["--remote", "ftp://box.example"]), no_env).is_err());
        assert!(parse(args(&["--remote", "http://127.0.0.1:1234"]), no_env).is_ok());
        assert!(parse(args(&["--remote", "http://[::1]:1234"]), no_env).is_ok());
        assert!(parse(args(&["--remote", "http://localhost:1234"]), no_env).is_ok());
    }

    #[test]
    fn embedded_takes_a_port() {
        assert!(matches!(
            parse(args(&["--embedded", "--port", "9999"]), no_env).unwrap(),
            Mode::Embedded { port: Some(9999) }
        ));
        assert!(parse(args(&["--embedded", "--port", "x"]), no_env).is_err());
    }

    #[test]
    fn bench_takes_a_page_and_output() {
        let Mode::Bench { page, out } =
            parse(args(&["--bench", "canvas", "--out", "/tmp/r.json"]), no_env).unwrap()
        else {
            panic!("expected bench");
        };
        assert_eq!(page, BenchPage::Canvas);
        assert_eq!(out, Some(PathBuf::from("/tmp/r.json")));
    }

    #[test]
    fn rejects_unknown_and_incomplete_arguments() {
        assert!(parse(args(&["--bogus"]), no_env).is_err());
        assert!(parse(args(&["--remote"]), no_env).is_err());
        assert!(parse(args(&["--bench", "nope"]), no_env).is_err());
    }
}
