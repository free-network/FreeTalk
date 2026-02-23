use base64::{engine::general_purpose::STANDARD, Engine};
use identicon_rs::Identicon;
use river_core::room_state::member::MemberId;

/// Generate an avatar as a base64-encoded PNG data URL from a MemberId.
/// The avatar is generated using the identicon-rs crate based on the member's ID bytes.
pub fn get_avatar(member_id: &MemberId) -> String {
    // Convert MemberId (which wraps FastHash(u64)) to bytes for identicon generation
    let id_bytes = (member_id.0).0.to_le_bytes();

    // Create a string representation for the identicon
    let id_string = bs58::encode(&id_bytes).into_string();

    // Generate identicon
    let identicon = Identicon::new(&id_string);

    // Export as base64 PNG and create data URI
    match identicon.export_png_data() {
        Ok(png_data) => {
            let base64_data = STANDARD.encode(&png_data);
            format!("data:image/png;base64,{}", base64_data)
        }
        Err(_) => {
            // Fallback to a simple colored placeholder
            String::from("data:image/svg+xml,%3Csvg xmlns='http://www.w3.org/2000/svg' width='40' height='40'%3E%3Crect fill='%23888' width='40' height='40'/%3E%3C/svg%3E")
        }
    }
}
