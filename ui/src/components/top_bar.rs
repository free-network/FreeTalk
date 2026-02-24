//! Top bar component showing user profile and admin controls.

use crate::components::app::{CURRENT_ROOM, MEMBER_INFO_MODAL, ROOMS};
use crate::util::avatar::get_avatar;
use crate::util::ecies::unseal_bytes_with_secrets;
use dioxus::prelude::*;
use river_core::room_state::member::MemberId;

/// Top bar component displaying the current user's profile and admin controls.
#[component]
pub fn TopBar() -> Element {
    // Get current room data
    let current_room_data = use_memo(move || {
        CURRENT_ROOM
            .read()
            .owner_key
            .and_then(|key| ROOMS.read().map.get(&key).cloned())
    });

    // Don't render if no room is selected
    let Some(room_data) = current_room_data.read().clone() else {
        return rsx! {};
    };

    let self_member_id = MemberId::from(&room_data.self_sk.verifying_key());
    let owner_id = MemberId::from(&room_data.owner_vk);
    let is_owner = self_member_id == owner_id;

    let self_nickname = room_data
        .room_state
        .member_info
        .member_info
        .iter()
        .find(|ami| ami.member_info.member_id == self_member_id)
        .map(|ami| {
            match unseal_bytes_with_secrets(&ami.member_info.preferred_nickname, &room_data.secrets)
            {
                Ok(bytes) => String::from_utf8_lossy(&bytes).to_string(),
                Err(_) => ami.member_info.preferred_nickname.to_string_lossy(),
            }
        })
        .unwrap_or_else(|| "You".to_string());

    let self_avatar = get_avatar(&self_member_id);
    let room_id = bs58::encode(room_data.owner_vk.as_bytes()).into_string();

    rsx! {
        div { class: "flex justify-between items-center bg-panel border-b border-border",
            // User profile header
            div {
                class: "flex items-center gap-3 px-6 py-4 cursor-pointer hover:bg-surface/50 transition-colors",
                onclick: move |_| {
                    MEMBER_INFO_MODAL.with_mut(|signal| {
                        signal.member = Some(self_member_id);
                    });
                },
                img {
                    src: "{self_avatar}",
                    alt: "Your avatar",
                    class: "w-12 h-12 rounded-full"
                }
                span { class: "text-xl font-medium text-text",
                    "{self_nickname}"
                }
                if is_owner {
                    span { class: "text-lg", title: "Board Owner", "👑" }
                }
            }

            // Admin button for owners
            if is_owner {
                a {
                    href: "#/room/{room_id}/admin",
                    class: "flex items-center gap-2 px-4 py-2 mr-4 bg-surface hover:bg-surface-hover text-text rounded-lg transition-colors",
                    title: "Manage Admins",
                    span { "⚙" }
                    span { "Admin" }
                }
            }
        }
    }
}
