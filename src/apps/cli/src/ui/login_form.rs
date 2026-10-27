//! Full-viewport OpenBitFun account panel (Login / Account status).
//!
//! Opened by `/login`. When already logged in, shows account info and connected
//! devices instead of the credential form. The sign-in form is a small wizard
//! for relay-local accounts: pick the method (SMS verification code or account
//! password), enter the phone number, then the 6-digit code or the password.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::{
    layout::{Alignment, Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, Paragraph, Wrap},
    Frame,
};

use crate::ui::theme::{StyleKind, Theme};
use openbitfun_product_domains::account::{AccountDevice, AccountInfo};

/// Action returned after handling a key event.
#[derive(Debug, Clone)]
pub(crate) enum LoginFormAction {
    None,
    /// Close the panel (Esc on most views).
    Cancel,
    /// Request an SMS verification code for the entered phone number.
    RequestCode {
        phone: String,
    },
    /// Sign in with the phone number and the SMS verification code.
    LoginWithCode {
        phone: String,
        code: String,
    },
    /// Sign in with the phone number and the account password.
    LoginWithPassword {
        phone: String,
        password: String,
    },
    /// User requested logout from the account page.
    Logout,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PanelMode {
    Login,
    Account,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LoginStep {
    /// Choose SMS verification code or account password.
    Method,
    /// Enter the account phone number.
    Phone,
    /// Enter the 6-digit SMS verification code.
    Code,
    /// Enter the account password.
    Password,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LoginMethod {
    SmsCode,
    Password,
}

impl LoginMethod {
    fn label(self) -> &'static str {
        match self {
            Self::SmsCode => "SMS verification code",
            Self::Password => "Account password",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AccountFocus {
    Logout,
    Close,
}

/// Full-screen account panel state.
pub(crate) struct LoginFormState {
    visible: bool,
    mode: PanelMode,

    step: LoginStep,
    method: LoginMethod,
    phone_buf: String,
    phone_cursor: usize,
    code_buf: String,
    code_cursor: usize,
    password_buf: String,
    password_cursor: usize,
    error: Option<String>,
    status: Option<String>,

    // Account status
    account_focus: AccountFocus,
    account_info: Option<AccountInfo>,
    devices: Vec<AccountDevice>,
}

impl LoginFormState {
    pub(crate) fn new() -> Self {
        Self {
            visible: false,
            mode: PanelMode::Login,
            step: LoginStep::Method,
            method: LoginMethod::SmsCode,
            phone_buf: String::new(),
            phone_cursor: 0,
            code_buf: String::new(),
            code_cursor: 0,
            password_buf: String::new(),
            password_cursor: 0,
            error: None,
            status: None,
            account_focus: AccountFocus::Close,
            account_info: None,
            devices: Vec::new(),
        }
    }

    pub(crate) fn is_visible(&self) -> bool {
        self.visible
    }

    pub(crate) fn show(&mut self) {
        self.visible = true;
        self.mode = PanelMode::Login;
        self.step = LoginStep::Method;
        self.method = LoginMethod::SmsCode;
        self.phone_buf.clear();
        self.phone_cursor = 0;
        self.code_buf.clear();
        self.code_cursor = 0;
        self.password_buf.clear();
        self.password_cursor = 0;
        self.error = None;
        self.status = None;
        self.account_focus = AccountFocus::Close;
    }

    pub(crate) fn hide(&mut self) {
        self.visible = false;
        self.password_buf.clear();
        self.password_cursor = 0;
        self.error = None;
        self.status = None;
    }

    pub(crate) fn show_account(&mut self, info: AccountInfo, devices: Vec<AccountDevice>) {
        self.visible = true;
        self.mode = PanelMode::Account;
        self.account_info = Some(info);
        self.devices = devices;
        self.account_focus = AccountFocus::Close;
        self.error = None;
        self.status = None;
    }

    pub(crate) fn set_error(&mut self, message: impl Into<String>) {
        self.error = Some(message.into());
        self.status = None;
    }

    pub(crate) fn set_status(&mut self, message: impl Into<String>) {
        self.status = Some(message.into());
        self.error = None;
    }

    pub(crate) fn insert_paste(&mut self, text: &str) {
        if !self.visible || self.mode != PanelMode::Login {
            return;
        }
        match self.step {
            LoginStep::Method => {}
            LoginStep::Phone => {
                let cleaned: String = text
                    .chars()
                    .filter(|c| *c != '\n' && *c != '\r' && *c != '\t')
                    .collect();
                for c in cleaned.chars() {
                    insert_char(&mut self.phone_buf, &mut self.phone_cursor, c);
                }
            }
            LoginStep::Code => {
                let digits: String = text
                    .chars()
                    .filter(|c| c.is_ascii_digit() && *c != '\n' && *c != '\r' && *c != '\t')
                    .take(8usize.saturating_sub(self.code_buf.chars().count()))
                    .collect();
                for c in digits.chars() {
                    insert_char(&mut self.code_buf, &mut self.code_cursor, c);
                }
            }
            LoginStep::Password => {
                let cleaned: String = text
                    .chars()
                    .filter(|c| *c != '\n' && *c != '\r' && *c != '\t')
                    .collect();
                for c in cleaned.chars() {
                    insert_char(&mut self.password_buf, &mut self.password_cursor, c);
                }
            }
        }
    }

    pub(crate) fn handle_key_event(&mut self, key: KeyEvent) -> LoginFormAction {
        if !self.visible {
            return LoginFormAction::None;
        }
        match self.mode {
            PanelMode::Login => self.handle_login_key(key),
            PanelMode::Account => self.handle_account_key(key),
        }
    }

    fn handle_login_key(&mut self, key: KeyEvent) -> LoginFormAction {
        match self.step {
            LoginStep::Method => self.handle_method_step(key),
            LoginStep::Phone => self.handle_phone_step(key),
            LoginStep::Code => self.handle_code_step(key),
            LoginStep::Password => self.handle_password_step(key),
        }
    }

    fn handle_method_step(&mut self, key: KeyEvent) -> LoginFormAction {
        match key.code {
            KeyCode::Esc => {
                self.hide();
                LoginFormAction::Cancel
            }
            KeyCode::Enter => {
                self.step = LoginStep::Phone;
                LoginFormAction::None
            }
            KeyCode::Left | KeyCode::Up | KeyCode::Char('h') | KeyCode::Char('k') => {
                self.method = LoginMethod::SmsCode;
                LoginFormAction::None
            }
            KeyCode::Right | KeyCode::Down | KeyCode::Char('l') | KeyCode::Char('j') => {
                self.method = LoginMethod::Password;
                LoginFormAction::None
            }
            KeyCode::Tab => {
                self.method = match self.method {
                    LoginMethod::SmsCode => LoginMethod::Password,
                    LoginMethod::Password => LoginMethod::SmsCode,
                };
                LoginFormAction::None
            }
            _ => LoginFormAction::None,
        }
    }

    fn handle_phone_step(&mut self, key: KeyEvent) -> LoginFormAction {
        match key.code {
            KeyCode::Esc => {
                self.step = LoginStep::Method;
                LoginFormAction::None
            }
            KeyCode::Enter => {
                let phone = self.phone_buf.trim().to_string();
                if phone.is_empty() {
                    self.set_error("Phone number cannot be empty.");
                    return LoginFormAction::None;
                }
                match self.method {
                    LoginMethod::SmsCode => {
                        self.step = LoginStep::Code;
                        self.code_buf.clear();
                        self.code_cursor = 0;
                        self.set_status(format!("Requesting a verification code for {phone}..."));
                        LoginFormAction::RequestCode { phone }
                    }
                    LoginMethod::Password => {
                        self.step = LoginStep::Password;
                        LoginFormAction::None
                    }
                }
            }
            KeyCode::Char('u') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.phone_buf.clear();
                self.phone_cursor = 0;
                LoginFormAction::None
            }
            KeyCode::Char(c) => {
                insert_char(&mut self.phone_buf, &mut self.phone_cursor, c);
                LoginFormAction::None
            }
            KeyCode::Backspace => {
                backspace(&mut self.phone_buf, &mut self.phone_cursor);
                LoginFormAction::None
            }
            KeyCode::Delete => {
                delete_forward(&mut self.phone_buf, &mut self.phone_cursor);
                LoginFormAction::None
            }
            KeyCode::Left => {
                self.phone_cursor = self.phone_cursor.saturating_sub(1);
                LoginFormAction::None
            }
            KeyCode::Right => {
                let max = self.phone_buf.chars().count();
                self.phone_cursor = (self.phone_cursor + 1).min(max);
                LoginFormAction::None
            }
            KeyCode::Home => {
                self.phone_cursor = 0;
                LoginFormAction::None
            }
            KeyCode::End => {
                self.phone_cursor = self.phone_buf.chars().count();
                LoginFormAction::None
            }
            _ => LoginFormAction::None,
        }
    }

    fn handle_code_step(&mut self, key: KeyEvent) -> LoginFormAction {
        match key.code {
            KeyCode::Esc => {
                self.step = LoginStep::Phone;
                LoginFormAction::None
            }
            KeyCode::Enter => {
                let phone = self.phone_buf.trim().to_string();
                let code = self.code_buf.trim().to_string();
                if code.is_empty() {
                    self.set_error("Enter the 6-digit verification code from the SMS.");
                    return LoginFormAction::None;
                }
                LoginFormAction::LoginWithCode { phone, code }
            }
            KeyCode::Char('u') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.code_buf.clear();
                self.code_cursor = 0;
                LoginFormAction::None
            }
            KeyCode::Char(c) if c.is_ascii_digit() => {
                if self.code_buf.chars().count() < 8 {
                    insert_char(&mut self.code_buf, &mut self.code_cursor, c);
                }
                LoginFormAction::None
            }
            KeyCode::Backspace => {
                backspace(&mut self.code_buf, &mut self.code_cursor);
                LoginFormAction::None
            }
            KeyCode::Delete => {
                delete_forward(&mut self.code_buf, &mut self.code_cursor);
                LoginFormAction::None
            }
            KeyCode::Left => {
                self.code_cursor = self.code_cursor.saturating_sub(1);
                LoginFormAction::None
            }
            KeyCode::Right => {
                let max = self.code_buf.chars().count();
                self.code_cursor = (self.code_cursor + 1).min(max);
                LoginFormAction::None
            }
            KeyCode::Home => {
                self.code_cursor = 0;
                LoginFormAction::None
            }
            KeyCode::End => {
                self.code_cursor = self.code_buf.chars().count();
                LoginFormAction::None
            }
            _ => LoginFormAction::None,
        }
    }

    fn handle_password_step(&mut self, key: KeyEvent) -> LoginFormAction {
        match key.code {
            KeyCode::Esc => {
                self.step = LoginStep::Phone;
                LoginFormAction::None
            }
            KeyCode::Enter => {
                let phone = self.phone_buf.trim().to_string();
                let password = self.password_buf.clone();
                if password.trim().is_empty() {
                    self.set_error("Password cannot be empty.");
                    return LoginFormAction::None;
                }
                LoginFormAction::LoginWithPassword { phone, password }
            }
            KeyCode::Char('u') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.password_buf.clear();
                self.password_cursor = 0;
                LoginFormAction::None
            }
            KeyCode::Char(c) => {
                insert_char(&mut self.password_buf, &mut self.password_cursor, c);
                LoginFormAction::None
            }
            KeyCode::Backspace => {
                backspace(&mut self.password_buf, &mut self.password_cursor);
                LoginFormAction::None
            }
            KeyCode::Delete => {
                delete_forward(&mut self.password_buf, &mut self.password_cursor);
                LoginFormAction::None
            }
            KeyCode::Left => {
                self.password_cursor = self.password_cursor.saturating_sub(1);
                LoginFormAction::None
            }
            KeyCode::Right => {
                let max = self.password_buf.chars().count();
                self.password_cursor = (self.password_cursor + 1).min(max);
                LoginFormAction::None
            }
            KeyCode::Home => {
                self.password_cursor = 0;
                LoginFormAction::None
            }
            KeyCode::End => {
                self.password_cursor = self.password_buf.chars().count();
                LoginFormAction::None
            }
            _ => LoginFormAction::None,
        }
    }

    fn handle_account_key(&mut self, key: KeyEvent) -> LoginFormAction {
        match (key.code, key.modifiers) {
            (KeyCode::Esc, _) => {
                self.hide();
                LoginFormAction::Cancel
            }
            (KeyCode::Up, _) | (KeyCode::BackTab, _) | (KeyCode::Left, _) => {
                self.account_focus = match self.account_focus {
                    AccountFocus::Logout => AccountFocus::Close,
                    AccountFocus::Close => AccountFocus::Logout,
                };
                LoginFormAction::None
            }
            (KeyCode::Down, _) | (KeyCode::Tab, _) | (KeyCode::Right, _) => {
                self.account_focus = match self.account_focus {
                    AccountFocus::Logout => AccountFocus::Close,
                    AccountFocus::Close => AccountFocus::Logout,
                };
                LoginFormAction::None
            }
            (KeyCode::Enter, _) => match self.account_focus {
                AccountFocus::Logout => LoginFormAction::Logout,
                AccountFocus::Close => {
                    self.hide();
                    LoginFormAction::Cancel
                }
            },
            _ => LoginFormAction::None,
        }
    }

    pub(crate) fn render(&self, frame: &mut Frame, area: Rect, theme: &Theme) {
        if !self.visible {
            return;
        }
        frame.render_widget(Clear, area);
        match self.mode {
            PanelMode::Login => self.render_login(frame, area, theme),
            PanelMode::Account => self.render_account(frame, area, theme),
        }
    }

    fn render_login(&self, frame: &mut Frame, area: Rect, theme: &Theme) {
        let outer = Block::default()
            .borders(Borders::ALL)
            .border_style(theme.style(StyleKind::Primary))
            .title(" OpenBitFun · Account Sign-in ")
            .title_alignment(Alignment::Center);
        let inner = outer.inner(area);
        frame.render_widget(outer, area);
        let rows = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(2),
                Constraint::Min(6),
                Constraint::Length(2),
                Constraint::Length(1),
            ])
            .split(inner);
        frame.render_widget(
            Paragraph::new(
                "Sign in with the phone number of your OpenBitFun account on the configured relay. First use registers the account.",
            )
            .style(theme.style(StyleKind::Muted))
            .wrap(Wrap { trim: false }),
            rows[0],
        );

        let mut lines: Vec<Line> = Vec::new();
        // Completed-step summary rows.
        if self.step != LoginStep::Method {
            lines.push(summary_line("Method", self.method.label(), theme));
        }
        if matches!(self.step, LoginStep::Code | LoginStep::Password) {
            lines.push(summary_line("Phone", self.phone_buf.trim(), theme));
        }
        if !lines.is_empty() {
            lines.push(Line::from(""));
        }

        let input_width = rows[1].width as usize;
        let hints: &str = match self.step {
            LoginStep::Method => {
                lines.push(prompt_line("Choose how to sign in:", theme));
                let sms_style = selector_style(self.method == LoginMethod::SmsCode, theme);
                let password_style = selector_style(self.method == LoginMethod::Password, theme);
                lines.push(Line::from(vec![
                    Span::raw("  "),
                    Span::styled(" > SMS verification code ", sms_style),
                    Span::raw("   "),
                    Span::styled(" > Account password ", password_style),
                ]));
                "Tab: Switch  Enter: Next  Esc: Close"
            }
            LoginStep::Phone => {
                lines.push(prompt_line("Phone number of the account:", theme));
                lines.push(render_input_line(
                    "  > ",
                    &self.phone_buf,
                    self.phone_cursor,
                    "+8613800138000",
                    false,
                    input_width,
                    theme,
                ));
                "Enter: Next  Ctrl+U: Clear  Esc: Back"
            }
            LoginStep::Code => {
                lines.push(prompt_line("Verification code (6 digits):", theme));
                lines.push(render_input_line(
                    "  > ",
                    &self.code_buf,
                    self.code_cursor,
                    "123456",
                    false,
                    input_width,
                    theme,
                ));
                "Enter: Sign in  Ctrl+U: Clear  Esc: Back"
            }
            LoginStep::Password => {
                lines.push(prompt_line("Account password:", theme));
                lines.push(render_input_line(
                    "  > ",
                    &self.password_buf,
                    self.password_cursor,
                    "not shown while typing",
                    true,
                    input_width,
                    theme,
                ));
                "Enter: Sign in  Ctrl+U: Clear  Esc: Back"
            }
        };
        frame.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }), rows[1]);
        self.render_message(frame, rows[2], theme);
        self.render_hints(frame, rows[3], hints, theme);
    }

    fn render_account(&self, frame: &mut Frame, area: Rect, theme: &Theme) {
        let outer = Block::default()
            .borders(Borders::ALL)
            .border_style(theme.style(StyleKind::Primary))
            .title(" OpenBitFun Account ")
            .title_alignment(Alignment::Center);
        let inner = outer.inner(area);
        frame.render_widget(outer, area);

        let rows = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(4), // account info
                Constraint::Min(4),    // devices
                Constraint::Length(1), // buttons
                Constraint::Length(1), // hints
            ])
            .split(inner);

        let info = self.account_info.as_ref();
        let info_lines = vec![
            Line::from(vec![
                Span::styled("User: ", theme.style(StyleKind::Muted)),
                Span::styled(
                    info.map(|i| i.user_id.as_str()).unwrap_or("-"),
                    Style::default()
                        .fg(Color::White)
                        .add_modifier(Modifier::BOLD),
                ),
            ]),
            Line::from(vec![
                Span::styled("Auth Server: ", theme.style(StyleKind::Muted)),
                Span::styled(
                    info.map(|i| i.relay_url.as_str()).unwrap_or("-"),
                    Style::default().fg(Color::White),
                ),
            ]),
            Line::from(vec![
                Span::styled("This device: ", theme.style(StyleKind::Muted)),
                Span::styled(
                    info.map(|i| i.device_name.as_str()).unwrap_or("-"),
                    Style::default().fg(Color::White),
                ),
                Span::styled(
                    format!(
                        "  ({})",
                        info.map(|i| truncate_id(&i.device_id))
                            .unwrap_or_else(|| "-".into())
                    ),
                    theme.style(StyleKind::Muted),
                ),
            ]),
        ];
        frame.render_widget(Paragraph::new(info_lines), rows[0]);

        let mut device_lines = vec![Line::from(Span::styled(
            "Devices",
            theme.style(StyleKind::Primary).add_modifier(Modifier::BOLD),
        ))];
        if self.devices.is_empty() {
            device_lines.push(Line::from(Span::styled(
                "  (no devices listed yet)",
                theme.style(StyleKind::Muted),
            )));
        } else {
            let local_id = info.map(|i| i.device_id.as_str());
            for d in &self.devices {
                let is_local = local_id == Some(d.device_id.as_str());
                let status = if d.online { "online" } else { "offline" };
                let badge = if is_local { " [this device]" } else { "" };
                // A missing flag (older Relay) is unknown and shows nothing.
                let compat = if d.is_compatible() {
                    ""
                } else {
                    "  · incompatible"
                };
                device_lines.push(Line::from(Span::styled(
                    format!(
                        "  {}{}  {}  · {}{}  {}",
                        d.display_name(),
                        badge,
                        truncate_id(&d.device_id),
                        status,
                        compat,
                        [
                            d.device_model.as_deref(),
                            d.device_os.as_deref(),
                            d.device_os_version.as_deref()
                        ]
                        .into_iter()
                        .flatten()
                        .collect::<Vec<_>>()
                        .join(" ")
                    ),
                    if d.online {
                        Style::default().fg(Color::White)
                    } else {
                        theme.style(StyleKind::Muted)
                    },
                )));
            }
        }
        frame.render_widget(Paragraph::new(device_lines), rows[1]);

        let btn_row = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Percentage(50), Constraint::Percentage(50)])
            .split(rows[2]);
        self.render_button(
            frame,
            btn_row[0],
            "[ Logout ]",
            self.account_focus == AccountFocus::Logout,
            theme,
        );
        self.render_button(
            frame,
            btn_row[1],
            "[ Close ]",
            self.account_focus == AccountFocus::Close,
            theme,
        );
        self.render_hints(
            frame,
            rows[3],
            "Tab Switch   Enter Activate   Esc Close",
            theme,
        );
    }

    fn render_button(
        &self,
        frame: &mut Frame,
        area: Rect,
        label: &str,
        active: bool,
        theme: &Theme,
    ) {
        let style = if active {
            Style::default()
                .bg(theme.primary)
                .fg(theme.selection_foreground())
                .add_modifier(Modifier::BOLD)
        } else {
            theme.style(StyleKind::Muted)
        };
        frame.render_widget(
            Paragraph::new(Line::from(Span::styled(label, style))).alignment(Alignment::Center),
            area,
        );
    }

    fn render_message(&self, frame: &mut Frame, area: Rect, theme: &Theme) {
        // The error may carry a second guidance line; keep it on its own row.
        let (message, style) = if let Some(ref err) = self.error {
            (err.as_str(), theme.style(StyleKind::Error))
        } else if let Some(ref status) = self.status {
            (status.as_str(), theme.style(StyleKind::Info))
        } else {
            return;
        };
        let lines: Vec<Line> = message
            .lines()
            .map(|line| Line::from(Span::styled(line.to_string(), style)))
            .collect();
        frame.render_widget(Paragraph::new(lines).alignment(Alignment::Center), area);
    }

    fn render_hints(&self, frame: &mut Frame, area: Rect, hints: &str, theme: &Theme) {
        frame.render_widget(
            Paragraph::new(Line::from(Span::styled(
                hints,
                theme.style(StyleKind::Muted),
            )))
            .alignment(Alignment::Center),
            area,
        );
    }
}

fn summary_line<'a>(label: &str, value: &'a str, theme: &Theme) -> Line<'a> {
    Line::from(vec![
        Span::styled(format!("{label}: "), theme.style(StyleKind::Muted)),
        Span::styled(
            value,
            theme.style(StyleKind::Primary).add_modifier(Modifier::BOLD),
        ),
    ])
}

fn prompt_line(prompt: &str, theme: &Theme) -> Line<'static> {
    Line::from(Span::styled(
        prompt.to_string(),
        theme.style(StyleKind::Info),
    ))
}

fn selector_style(active: bool, theme: &Theme) -> Style {
    if active {
        Style::default()
            .fg(theme.selection_foreground())
            .bg(theme.primary)
            .add_modifier(Modifier::BOLD)
    } else {
        theme.style(StyleKind::Muted)
    }
}

/// Render a single-line input field with cursor, placeholder, and masking.
/// Masked fields show one filler character per typed character.
fn render_input_line<'a>(
    label: &'a str,
    buffer: &'a str,
    cursor: usize,
    placeholder: &'a str,
    masked: bool,
    available_width: usize,
    theme: &Theme,
) -> Line<'a> {
    let display: String = if masked {
        "*".repeat(buffer.chars().count())
    } else {
        buffer.to_string()
    };
    let label_style = theme.style(StyleKind::Primary).add_modifier(Modifier::BOLD);
    let text_style = Style::default().fg(Color::White);

    let field_width = available_width.saturating_sub(label.chars().count());
    if buffer.is_empty() {
        let placeholder_display: String = placeholder
            .chars()
            .take(field_width.saturating_sub(1))
            .collect();
        return Line::from(vec![
            Span::styled(label, label_style),
            Span::styled(" ", Style::default().fg(Color::Black).bg(Color::White)),
            Span::styled(
                placeholder_display,
                theme.style(StyleKind::Muted).add_modifier(Modifier::DIM),
            ),
        ]);
    }

    let scroll = if field_width == 0 || cursor < field_width / 3 {
        0
    } else {
        cursor.saturating_sub(field_width / 3)
    };
    let visible_chars: String = display.chars().skip(scroll).take(field_width).collect();
    let cursor_in_view = cursor.saturating_sub(scroll);
    let before: String = visible_chars.chars().take(cursor_in_view).collect();
    let cursor_char: String = visible_chars.chars().skip(cursor_in_view).take(1).collect();
    let after: String = visible_chars.chars().skip(cursor_in_view + 1).collect();
    let cursor_display = if cursor_char.is_empty() {
        " ".to_string()
    } else {
        cursor_char
    };
    Line::from(vec![
        Span::styled(label, label_style),
        Span::styled(before, text_style),
        Span::styled(
            cursor_display,
            Style::default().fg(Color::Black).bg(Color::White),
        ),
        Span::styled(after, text_style),
    ])
}

fn truncate_id(id: &str) -> String {
    if id.len() <= 8 {
        id.to_string()
    } else {
        format!("{}…", &id[..8])
    }
}

fn insert_char(buf: &mut String, cursor: &mut usize, c: char) {
    let byte_pos = char_to_byte(buf, *cursor);
    buf.insert(byte_pos, c);
    *cursor += 1;
}

fn backspace(buf: &mut String, cursor: &mut usize) {
    if *cursor > 0 {
        *cursor -= 1;
        let byte_pos = char_to_byte(buf, *cursor);
        let next_byte = char_to_byte(buf, *cursor + 1);
        buf.replace_range(byte_pos..next_byte, "");
    }
}

fn delete_forward(buf: &mut String, cursor: &mut usize) {
    let max = buf.chars().count();
    if *cursor < max {
        let byte_pos = char_to_byte(buf, *cursor);
        let next_byte = char_to_byte(buf, *cursor + 1);
        buf.replace_range(byte_pos..next_byte, "");
    }
}

fn char_to_byte(s: &str, char_pos: usize) -> usize {
    s.char_indices()
        .nth(char_pos)
        .map(|(i, _)| i)
        .unwrap_or(s.len())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

    fn press(form: &mut LoginFormState, code: KeyCode) -> LoginFormAction {
        form.handle_key_event(KeyEvent::new(code, KeyModifiers::NONE))
    }

    fn type_text(form: &mut LoginFormState, text: &str) {
        for c in text.chars() {
            form.handle_key_event(KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE));
        }
    }

    #[test]
    fn sms_flow_requests_a_code_then_logs_in_with_the_code() {
        let mut form = LoginFormState::new();
        form.show();
        assert!(matches!(
            press(&mut form, KeyCode::Enter),
            LoginFormAction::None
        ));
        type_text(&mut form, "+8613800138000");
        match press(&mut form, KeyCode::Enter) {
            LoginFormAction::RequestCode { phone } => assert_eq!(phone, "+8613800138000"),
            other => panic!("expected RequestCode, got {other:?}"),
        }
        // Non-digit characters never enter the code field.
        type_text(&mut form, "12a4");
        assert_eq!(form.code_buf, "124");
        match press(&mut form, KeyCode::Enter) {
            LoginFormAction::LoginWithCode { phone, code } => {
                assert_eq!(phone, "+8613800138000");
                assert_eq!(code, "124");
            }
            other => panic!("expected LoginWithCode, got {other:?}"),
        }
    }

    #[test]
    fn password_flow_submits_phone_and_password() {
        let mut form = LoginFormState::new();
        form.show();
        press(&mut form, KeyCode::Tab);
        press(&mut form, KeyCode::Enter);
        type_text(&mut form, "+8613800138000");
        // Enter on the phone step only advances to the password step.
        assert!(matches!(
            press(&mut form, KeyCode::Enter),
            LoginFormAction::None
        ));
        type_text(&mut form, "hunter2");
        match press(&mut form, KeyCode::Enter) {
            LoginFormAction::LoginWithPassword { phone, password } => {
                assert_eq!(phone, "+8613800138000");
                assert_eq!(password, "hunter2");
            }
            other => panic!("expected LoginWithPassword, got {other:?}"),
        }
    }

    #[test]
    fn empty_phone_and_password_are_rejected_without_actions() {
        let mut form = LoginFormState::new();
        form.show();
        press(&mut form, KeyCode::Enter);
        assert!(matches!(
            press(&mut form, KeyCode::Enter),
            LoginFormAction::None
        ));
        assert!(form.error.is_some());
        assert!(form.phone_buf.is_empty());

        type_text(&mut form, "+8613800138000");
        press(&mut form, KeyCode::Esc); // back to the method step
        press(&mut form, KeyCode::Tab); // switch to password method
        press(&mut form, KeyCode::Enter); // method -> phone
        press(&mut form, KeyCode::Enter); // phone -> password
        assert!(matches!(
            press(&mut form, KeyCode::Enter),
            LoginFormAction::None
        ));
        assert!(form.error.is_some());
    }

    #[test]
    fn esc_walks_back_through_steps_and_cancels_at_the_root() {
        let mut form = LoginFormState::new();
        form.show();
        press(&mut form, KeyCode::Enter);
        type_text(&mut form, "+8613800138000");
        assert!(matches!(
            press(&mut form, KeyCode::Enter),
            LoginFormAction::RequestCode { .. }
        ));
        assert_eq!(form.step, LoginStep::Code);
        press(&mut form, KeyCode::Esc);
        assert_eq!(form.step, LoginStep::Phone);
        press(&mut form, KeyCode::Esc);
        assert_eq!(form.step, LoginStep::Method);
        assert!(matches!(
            press(&mut form, KeyCode::Esc),
            LoginFormAction::Cancel
        ));
        assert!(!form.is_visible());
    }

    #[test]
    fn paste_lands_in_the_active_field_and_hide_never_leaks_password() {
        let mut form = LoginFormState::new();
        form.show();
        press(&mut form, KeyCode::Enter);
        form.insert_paste("+8613800138000");
        assert_eq!(form.phone_buf, "+8613800138000");
        press(&mut form, KeyCode::Enter);
        form.insert_paste("123456");
        assert_eq!(form.code_buf, "123456");

        press(&mut form, KeyCode::Esc);
        press(&mut form, KeyCode::Esc);
        press(&mut form, KeyCode::Tab); // switch to password method
        press(&mut form, KeyCode::Enter); // back to the phone step (buffer kept)
        press(&mut form, KeyCode::Enter); // phone -> password step
        form.insert_paste("secret");
        assert_eq!(form.password_buf, "secret");
        form.hide();
        assert!(
            form.password_buf.is_empty(),
            "password must not survive hide"
        );
    }
}
