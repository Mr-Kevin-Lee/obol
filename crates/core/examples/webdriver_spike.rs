//! fantoccini/WebDriver spike (spec §14, §15, task 21) — a go/no-go
//! checkpoint per §5's explicit carve-out (manual verification, not
//! TDD), now unblocked by a concrete real target: Vanguard, confirmed
//! unreachable via Plaid at all (D25).
//!
//! **Hand-off design (decision D27, §16):** rather than scripting the
//! login itself, this opens a real browser window and lets a human log
//! in manually, then takes over the *same authenticated session*
//! afterward. Discovered why the hard way — Vanguard's login button had
//! a duplicate DOM id (an outer `<c11n-button>` custom element and the
//! real inner `<button>` both used
//! `id="username-password-submit-btn-1"`), so a scripted click hit the
//! wrong element; more fundamentally, fighting a modern Angular login
//! form (plus whatever MFA/CAPTCHA it might show) is exactly the kind
//! of fragile, anti-automation-hardened surface this project doesn't
//! need to automate at all, since a human is always present when this
//! tool runs anyway.
//!
//! Uses `safaridriver` rather than chromedriver — the WebDriver
//! protocol itself isn't browser-specific (fantoccini just speaks W3C
//! WebDriver to whatever's listening on the given port), and
//! `safaridriver` ships built into macOS, always version-matched to
//! whatever Safari version is installed. No separate driver binary to
//! install or keep in sync with browser updates, unlike chromedriver.
//!
//! One-time setup:
//!   1. Safari → Settings → Advanced → check "Show features for web
//!      developers" (unlocks the Develop menu).
//!   2. Safari → Develop menu → check "Allow Remote Automation".
//!   3. `safaridriver --enable` (one-time, prompts for your password).
//!
//! Then, each time:
//!   safaridriver -p 9515
//!
//! Run:
//!   VANGUARD_LOGIN_URL=https://... \
//!     cargo run -p obol-core --example webdriver_spike
//!
//! No credentials are typed into this program at all — you log in
//! directly in the real browser window, so your username/password never
//! pass through this code or get held in memory here.

use fantoccini::{ClientBuilder, Locator};
use std::io::Write;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // rustls 0.23+ needs an explicit process-level CryptoProvider when
    // more than one backend could apply — fantoccini's rustls-tls
    // feature (via hyper-rustls) doesn't pick one for us the way
    // reqwest's rustls-tls integration does internally.
    rustls::crypto::ring::default_provider()
        .install_default()
        .expect("failed to install rustls CryptoProvider");

    let login_url = std::env::var("VANGUARD_LOGIN_URL")
        .expect("set VANGUARD_LOGIN_URL to your bank's real login page");

    println!("Connecting to WebDriver server at http://localhost:9515...");
    let client = ClientBuilder::rustls()?
        .connect("http://localhost:9515")
        .await?;

    println!("Navigating to {login_url}...");
    client.goto(&login_url).await?;

    print!(
        "\nLog in to Vanguard yourself in the browser window that just opened \
         (username, password, MFA — whatever it asks for). Once you're fully \
         logged in and looking at your account/dashboard page, press Enter \
         here: "
    );
    std::io::stdout().flush()?;
    let mut _input = String::new();
    std::io::stdin().read_line(&mut _input)?;

    println!("\nTaking over the session now that you're logged in.");
    println!("Current title: {:?}", client.title().await?);
    println!("Current URL: {:?}", client.current_url().await?);
    println!(
        "\nNavigate (in that same browser window) to wherever your balance is \
         shown, then press Enter again — next step is finding the right \
         selector to actually read it: "
    );
    let mut _input2 = String::new();
    std::io::stdin().read_line(&mut _input2)?;

    println!("Title on the balance page: {:?}", client.title().await?);
    println!("URL on the balance page: {:?}", client.current_url().await?);

    // Vanguard shows a marketing interstitial (a c-layer-offer modal) on
    // most authenticated page loads, sitting on top of everything else.
    // Best-effort dismiss: if it's not there (already closed, or a
    // different page entirely), finding it just fails and we move on.
    if let Ok(close_button) = client.find(Locator::Css(".layer-close")).await {
        let _ = close_button.click().await;
    }

    // The dashboard's greeting widget carries the total balance as a
    // plain HTML attribute on a custom element — no text-scraping or
    // number-parsing needed, and far less likely to break across
    // Vanguard's frequent front-end redesigns than a CSS class or DOM
    // path would be.
    let greeting = client.find(Locator::Css("gyd-greetings-widget")).await?;
    let balance = greeting.attr("current-balance").await?;
    let as_of = greeting.attr("value-as-of-date").await?;
    println!("\nBalance found: {balance:?}");
    println!("As of: {as_of:?}");

    print!("\nPress Enter once more to close the browser: ");
    std::io::stdout().flush()?;
    let mut _input3 = String::new();
    std::io::stdin().read_line(&mut _input3)?;

    client.close().await?;
    Ok(())
}
