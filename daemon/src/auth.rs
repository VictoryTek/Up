use log::{info, warn};
use zbus::Connection;

/// Check polkit authorization for the given caller and action.
///
/// This calls the polkit Authority over D-Bus to verify the caller is
/// authorized to perform the specified action. The `AllowUserInteraction`
/// flag is set so the polkit agent can prompt the user if needed.
pub async fn check_polkit(
    connection: &Connection,
    caller_bus_name: &str,
    action_id: &str,
) -> Result<bool, String> {
    info!(
        "Checking polkit auth: caller={}, action={}",
        caller_bus_name, action_id
    );

    // Build the subject identifying the caller by their system bus name
    let subject = (
        "system-bus-name",
        std::collections::HashMap::from([("name", zbus::zvariant::Value::from(caller_bus_name))]),
    );

    // Connect to the polkit authority
    let polkit = zbus::proxy::Builder::<PolkitAuthorityProxy>::new(connection)
        .destination("org.freedesktop.PolicyKit1")
        .map_err(|e| format!("Failed to set polkit destination: {}", e))?
        .path("/org/freedesktop/PolicyKit1/Authority")
        .map_err(|e| format!("Failed to set polkit path: {}", e))?
        .build()
        .await
        .map_err(|e| format!("Failed to connect to polkit: {}", e))?;

    // CheckAuthorization with AllowUserInteraction (flag = 1)
    let result = polkit
        .check_authorization(
            &subject,
            action_id,
            &std::collections::HashMap::<&str, &str>::new(),
            1u32,
            "",
        )
        .await
        .map_err(|e| format!("Polkit CheckAuthorization failed: {}", e))?;

    if result.is_authorized {
        info!("Polkit authorized: action={}", action_id);
        Ok(true)
    } else {
        warn!("Polkit denied: action={}", action_id);
        Ok(false)
    }
}

/// Minimal polkit Authority proxy — just the CheckAuthorization method.
#[zbus::proxy(
    interface = "org.freedesktop.PolicyKit1.Authority",
    default_service = "org.freedesktop.PolicyKit1",
    default_path = "/org/freedesktop/PolicyKit1/Authority"
)]
trait PolkitAuthority {
    /// Check if a subject is authorized for an action.
    async fn check_authorization(
        &self,
        subject: &(
            &str,
            std::collections::HashMap<&str, zbus::zvariant::Value<'_>>,
        ),
        action_id: &str,
        details: &std::collections::HashMap<&str, &str>,
        flags: u32,
        cancellation_id: &str,
    ) -> zbus::Result<AuthorizationResult>;
}

/// The result from CheckAuthorization.
#[derive(Debug, serde::Deserialize, zbus::zvariant::Type)]
pub struct AuthorizationResult {
    pub is_authorized: bool,
    #[allow(dead_code)]
    pub is_challenge: bool,
    #[allow(dead_code)]
    pub details: std::collections::HashMap<String, String>,
}
