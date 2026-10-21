//! `rc models`: list the models an endpoint offers.

use std::io::Read;

use anyhow::{bail, Result};
use clap::Args;
use ruckcode_core::setup_api::{self, ModelsAuth};
use zeroize::Zeroize;

#[derive(Args, Debug, Default)]
pub struct ModelsArgs {
    /// `anthropic` or `openai` (OpenAI-compatible: OpenAI, Ollama, OpenRouter, vLLM, ...).
    #[arg(long)]
    pub provider: String,
    /// Endpoint (default: the provider's usual URL).
    #[arg(long)]
    pub base_url: Option<String>,
    /// Read the key from this environment variable.
    #[arg(long, conflicts_with_all = ["api_key_name", "api_key_stdin"])]
    pub api_key_env: Option<String>,
    /// Use this named key from credentials.toml.
    #[arg(long, conflicts_with = "api_key_stdin")]
    pub api_key_name: Option<String>,
    /// Read a key that is not saved yet from stdin (it is never stored).
    #[arg(long)]
    pub api_key_stdin: bool,
    /// JSON: `{"models":[{"id","displayName"}]}`.
    #[arg(long)]
    pub json: bool,
}

/// The auth choice of the flags; `stdin` supplies the secret for `--api-key-stdin`.
pub fn auth_from_args(
    args: &ModelsArgs,
    stdin: impl FnOnce() -> Result<String>,
) -> Result<Option<ModelsAuth>> {
    Ok(if args.api_key_stdin {
        let mut s = stdin()?;
        let t = s.trim().to_string();
        s.zeroize();
        if t.is_empty() {
            bail!("--api-key-stdin: no key on stdin");
        }
        Some(ModelsAuth::Secret(t))
    } else if let Some(n) = &args.api_key_name {
        Some(ModelsAuth::KeyName(n.clone()))
    } else {
        args.api_key_env.clone().map(ModelsAuth::Env)
    })
}

pub async fn run(args: &ModelsArgs) -> Result<()> {
    let auth = auth_from_args(args, || {
        let mut s = String::new();
        std::io::stdin().read_to_string(&mut s)?;
        Ok(s)
    })?;
    let models = setup_api::list_models(
        &args.provider,
        args.base_url.as_deref().unwrap_or(""),
        auth,
        None,
        &ruckcode_core::setup::process_env,
    )
    .await?;
    if args.json {
        println!("{}", serde_json::json!({ "models": models }));
    } else {
        for m in &models {
            match &m.display_name {
                Some(d) => println!("{}  ({d})", m.id),
                None => println!("{}", m.id),
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn auth_choices() {
        let base = ModelsArgs {
            provider: "openai".into(),
            ..Default::default()
        };
        assert!(auth_from_args(&base, || unreachable!()).unwrap().is_none());
        let a = ModelsArgs {
            api_key_stdin: true,
            ..Default::default()
        };
        match auth_from_args(&a, || Ok(" sk \n".into())).unwrap() {
            Some(ModelsAuth::Secret(ref s)) => assert_eq!(s, "sk"),
            o => panic!("{o:?}"),
        }
        assert!(auth_from_args(&a, || Ok("\n".into())).is_err());
        let n = ModelsArgs {
            api_key_name: Some("k".into()),
            ..Default::default()
        };
        assert!(matches!(
            auth_from_args(&n, || unreachable!()).unwrap(),
            Some(ModelsAuth::KeyName(_))
        ));
        let e = ModelsArgs {
            api_key_env: Some("V".into()),
            ..Default::default()
        };
        assert!(matches!(
            auth_from_args(&e, || unreachable!()).unwrap(),
            Some(ModelsAuth::Env(_))
        ));
    }
}
