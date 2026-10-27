//! Inspect, provision, and explicitly delete relay-local account records.
use anyhow::{anyhow, Result};
use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(name = "relay-admin", about = "Manage relay-local accounts")]
struct Cli {
    #[arg(long, env = "RELAY_DB_PATH")]
    db: String,
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// List local accounts (handle, user id, masked phone, created).
    ListUsers,
    /// Explicitly delete an account and its relay data.
    DeleteUser {
        #[arg(long)]
        username: Option<String>,
        #[arg(long)]
        phone: Option<String>,
    },
    /// Create a local account with an operator-verified phone. For relays
    /// running with self-registration closed.
    AddUser {
        #[arg(long)]
        phone: String,
        /// Read the optional password from stdin (pipe it in; input is not echoed).
        #[arg(long)]
        password_stdin: bool,
    },
    /// Set or replace an account's optional password, revoking its sessions.
    SetPassword {
        #[arg(long)]
        phone: String,
        /// Read the new password from stdin (pipe it in; input is not echoed).
        #[arg(long)]
        password_stdin: bool,
    },
}

fn read_stdin_line(prompt: &str) -> Result<String> {
    eprintln!("{prompt}");
    let mut line = String::new();
    std::io::stdin()
        .read_line(&mut line)
        .map_err(|e| anyhow!("read stdin: {e}"))?;
    Ok(line.trim_end_matches(['\r', '\n']).to_string())
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    let pool = openbitfun_relay_service::db::connect_for_admin(&cli.db).await?;
    match cli.command {
        Command::ListUsers => {
            for user in openbitfun_relay_service::admin::list_users(&pool).await? {
                println!(
                    "{}\t{}\t{}\t{}",
                    user.username,
                    user.user_id,
                    user.phone_masked.as_deref().unwrap_or("-"),
                    user.created_at
                );
            }
        }
        Command::DeleteUser { username, phone } => match (username, phone) {
            (Some(username), None) => {
                openbitfun_relay_service::admin::delete_user(&pool, &username).await?;
                println!("Deleted relay account: {username}");
            }
            (None, Some(phone)) => {
                openbitfun_relay_service::admin::delete_user_by_phone(&pool, &phone).await?;
                println!("Deleted relay account for {phone}");
            }
            _ => {
                return Err(anyhow!("pass exactly one of --username or --phone"));
            }
        },
        Command::AddUser {
            phone,
            password_stdin,
        } => {
            let password = if password_stdin {
                Some(read_stdin_line(
                    "Enter password (stdin, 8-128 bytes, empty for none):",
                )?)
            } else {
                None
            };
            let user_id =
                openbitfun_relay_service::admin::add_local_user(&pool, &phone, password.as_deref())
                    .await?;
            println!("Created local account {user_id} for {phone}");
        }
        Command::SetPassword {
            phone,
            password_stdin,
        } => {
            if !password_stdin {
                return Err(anyhow!("use --password-stdin and pipe the new password"));
            }
            let password = read_stdin_line("Enter new password (stdin):")?;
            openbitfun_relay_service::admin::set_password(&pool, &phone, &password).await?;
            println!("Password updated for {phone}; its sessions were revoked");
        }
    }
    Ok(())
}
