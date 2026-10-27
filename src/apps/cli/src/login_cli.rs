//! Plain-terminal account sign-in for the CLI (`openbitfun login`).
//!
//! Complements the TUI `/login` panel for hosts without a full TUI (SSH
//! sessions, scripts, CI): it signs a relay-local account in with an SMS
//! verification code or the account password. Interactive terminals prompt
//! for each credential; non-TTY contexts read credentials from flags, the
//! `OPENBITFUN_ACCOUNT_PASSWORD` environment variable, or stdin lines, and
//! fail loudly instead of hanging when no credential can be obtained.

use std::io::{IsTerminal, Write};

use anyhow::{anyhow, Context, Result};

/// Environment variable accepted as the account password in non-TTY contexts.
const PASSWORD_ENV: &str = "OPENBITFUN_ACCOUNT_PASSWORD";

pub(crate) struct LoginCommandInput {
    pub phone: Option<String>,
    pub code: Option<String>,
    pub password: Option<String>,
}

pub(crate) async fn run_login(input: LoginCommandInput) -> Result<()> {
    let runtime = crate::account::build_management_account_runtime();
    let interactive = std::io::stdin().is_terminal();

    let password_from_env = std::env::var(PASSWORD_ENV)
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty());
    if input.code.is_some() && (input.password.is_some() || password_from_env.is_some()) {
        return Err(anyhow!(
            "pass either an SMS verification code (--code) or a password (--password / {PASSWORD_ENV}), not both"
        ));
    }

    // Method selection: an explicit credential picks the method; a TTY asks.
    let use_password = if input.password.is_some() || password_from_env.is_some() {
        true
    } else if input.code.is_some() {
        false
    } else if interactive {
        prompt_use_password()?
    } else {
        false
    };

    let phone = match input.phone.as_deref() {
        Some(phone) => non_empty(phone.trim().to_string(), "Phone number")?,
        None if interactive => prompt_line("Phone number of the account")?,
        None => read_stdin_credential("Phone number")?,
    };

    let (code, password) = if use_password {
        let password = match (
            input.password.as_deref(),
            password_from_env.as_deref(),
            interactive,
        ) {
            (Some(password), _, _) | (_, Some(password), _) => {
                require_non_blank(password.to_string(), "Password")?
            }
            (None, None, true) => {
                let password = rpassword::prompt_password("  Account password: ")?;
                require_non_blank(password, "Password")?
            }
            (None, None, false) => {
                // Preserve the exact line; only reject blank input loudly.
                let mut buffer = String::new();
                let bytes = std::io::stdin()
                    .read_line(&mut buffer)
                    .context("Failed to read from stdin")?;
                if bytes == 0 || buffer.trim().is_empty() {
                    return Err(anyhow!(
                        "Password is required but none could be read from stdin. Run `openbitfun login` in an interactive terminal, or pass --password or {PASSWORD_ENV}"
                    ));
                }
                buffer.trim_end_matches(['\r', '\n']).to_string()
            }
        };
        (None, Some(password))
    } else {
        // Request a fresh code only when the caller did not already have one;
        // a relay-side cooldown would otherwise reject the request.
        let code = match input.code.as_deref() {
            Some(code) => non_empty(code.trim().to_string(), "Verification code")?,
            None => {
                println!("Requesting a verification code for {phone}...");
                runtime.local_sms_send(&phone).await.map_err(|error| {
                    anyhow!(
                        "could not send the verification code: {}",
                        crate::account::bounded_account_error(&error.to_string())
                    )
                })?;
                if interactive {
                    prompt_line("6-digit verification code from the SMS")?
                } else {
                    read_stdin_credential("Verification code")?
                }
            }
        };
        (Some(code), None)
    };

    let login = runtime
        .login_with_local_credentials(&phone, code.as_deref(), password.as_deref())
        .await
        .map_err(|error| {
            anyhow!(
                "login failed: {}",
                crate::account::bounded_account_error(&error.to_string())
            )
        })?;
    println!("{}", crate::account::account_login_status_message(&login));
    Ok(())
}

/// Ask on an interactive terminal whether to sign in with the account
/// password instead of an SMS verification code.
fn prompt_use_password() -> Result<bool> {
    print!("Sign in with [1] SMS verification code (default) or [2] account password: ");
    std::io::stdout()
        .flush()
        .context("Failed to flush stdout")?;
    let mut buffer = String::new();
    std::io::stdin()
        .read_line(&mut buffer)
        .context("Failed to read from stdin")?;
    parse_method_answer(&buffer)
}

/// Map the interactive method answer onto `true` for password sign-in.
fn parse_method_answer(answer: &str) -> Result<bool> {
    match answer.trim().to_lowercase().as_str() {
        "" | "1" | "sms" | "code" => Ok(false),
        "2" | "password" | "p" => Ok(true),
        other => Err(anyhow!(
            "choose 1 (SMS verification code) or 2 (account password), got `{other}`"
        )),
    }
}

/// Prompt for one plain credential line on an interactive terminal.
fn prompt_line(label: &str) -> Result<String> {
    print!("  {label}: ");
    std::io::stdout()
        .flush()
        .context("Failed to flush stdout")?;
    let mut buffer = String::new();
    std::io::stdin()
        .read_line(&mut buffer)
        .context("Failed to read from stdin")?;
    non_empty(buffer.trim().to_string(), label)
}

/// Read one credential line from stdin in a non-TTY context. Empty input or
/// closed stdin is a loud error that points at the interactive path; the
/// command never blocks waiting for a terminal that does not exist.
fn read_stdin_credential(label: &str) -> Result<String> {
    let mut buffer = String::new();
    let bytes = std::io::stdin()
        .read_line(&mut buffer)
        .context("Failed to read from stdin")?;
    if bytes == 0 || buffer.trim().is_empty() {
        return Err(anyhow!(
            "{label} is required but no {label} could be read from stdin. Run `openbitfun login` in an interactive terminal, or pass --phone plus --code, or --password / {PASSWORD_ENV}"
        ));
    }
    Ok(buffer.trim().to_string())
}

fn non_empty(value: String, label: &str) -> Result<String> {
    if value.trim().is_empty() {
        return Err(anyhow!("{label} cannot be empty"));
    }
    Ok(value.trim().to_string())
}

/// Validate a secret without normalizing it: the exact characters are kept.
fn require_non_blank(value: String, label: &str) -> Result<String> {
    if value.trim().is_empty() {
        return Err(anyhow!("{label} cannot be empty"));
    }
    Ok(value)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn non_empty_rejects_blank_credentials() {
        assert!(non_empty(String::new(), "Phone number").is_err());
        assert!(non_empty("  ".to_string(), "Password").is_err());
        assert_eq!(
            non_empty(" 123456 ".to_string(), "Verification code").unwrap(),
            "123456"
        );
    }

    #[test]
    fn secrets_keep_their_exact_characters() {
        assert_eq!(
            require_non_blank(" pass word ".to_string(), "Password").unwrap(),
            " pass word "
        );
        assert!(require_non_blank("\t".to_string(), "Password").is_err());
    }

    #[test]
    fn method_prompt_parses_choices() {
        assert!(!parse_method_answer("").unwrap());
        assert!(!parse_method_answer(" 1 ").unwrap());
        assert!(!parse_method_answer("SMS").unwrap());
        assert!(!parse_method_answer("code").unwrap());
        assert!(parse_method_answer("2").unwrap());
        assert!(parse_method_answer(" Password ").unwrap());
        assert!(parse_method_answer("P").unwrap());
        assert!(parse_method_answer("3").is_err());
    }
}
