use crate::components::app::{mark_needs_sync, BOARDS, CURRENT_BOARD, MEMBER_INFO_MODAL};
use crate::util::ecies::unseal_bytes_with_secrets;
use dioxus::prelude::*;
use dioxus_free_icons::icons::fa_solid_icons::{FaFileExport, FaFileImport, FaUserPlus, FaUsers};
use dioxus_free_icons::Icon;
use ed25519_dalek::{SigningKey, VerifyingKey};
use river_core::board_state::identity::IdentityExport;
use river_core::board_state::member::MembersV1;
use river_core::board_state::member::{AuthorizedMember, Member, MemberId};
use river_core::board_state::ChatBoardParametersV1;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};

pub mod invite_member_modal;
pub mod member_info_modal;
use self::invite_member_modal::InviteMemberModal;

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Invitation {
    pub board: VerifyingKey,
    pub invitee_signing_key: SigningKey,
    pub invitee: AuthorizedMember,
}

impl Invitation {
    /// Encode as base58 string
    pub fn to_encoded_string(&self) -> String {
        let mut data = Vec::new();
        ciborium::ser::into_writer(self, &mut data).expect("Serialization should not fail");
        bs58::encode(data).into_string()
    }

    /// Decode from base58 string
    pub fn from_encoded_string(s: &str) -> Result<Self, String> {
        let decoded = bs58::decode(s)
            .into_vec()
            .map_err(|e| format!("Base58 decode error: {}", e))?;
        ciborium::de::from_reader(&decoded[..]).map_err(|e| format!("Deserialization error: {}", e))
    }
}

struct MemberDisplay {
    nickname: String,
    _member_id: MemberId,
    is_owner: bool,
    is_admin: bool,
    is_self: bool,
    invited_you: bool,
    sponsored_you: bool,
    invited_by_you: bool,
    in_your_network: bool,
}

fn is_member_sponsor(
    member_id: MemberId,
    members: &MembersV1,
    self_id: MemberId,
    params: &ChatBoardParametersV1,
) -> bool {
    // Check if member is in invite chain but not direct inviter
    if let Some(self_member) = members.members.iter().find(|m| m.member.id() == self_id) {
        if let Ok(chain) = members.get_invite_chain(self_member, params) {
            return chain.iter().any(|m| m.member.id() == member_id);
        }
    }
    false
}

fn is_in_your_network(member_id: MemberId, members: &MembersV1, self_id: MemberId) -> bool {
    // Check if this member was invited by someone you invited
    members.members.iter().any(|m| {
        m.member.id() == member_id
            && members.members.iter().any(|inviter| {
                inviter.member.id() == m.member.invited_by
                    && did_you_invite_member(inviter.member.id(), members, self_id)
            })
    })
}

fn did_you_invite_member(member_id: MemberId, members: &MembersV1, self_id: MemberId) -> bool {
    members
        .members
        .iter()
        .find(|m| m.member.id() == member_id)
        .map(|m| m.member.invited_by == self_id)
        .unwrap_or(false)
}

fn format_member_display(member: &MemberDisplay) -> String {
    let mut tags: Vec<(&str, &str)> = Vec::new();

    if member.is_owner {
        tags.push(("👑", "Board Owner"));
    } else if member.is_admin {
        tags.push(("👑", "Admin"));
    }
    if member.is_self {
        tags.push(("⭐", "You"));
    }
    if member.invited_by_you {
        tags.push(("🔑", "Invited by You"));
    } else if member.in_your_network {
        tags.push(("🌐", "In Your Network"));
    }
    if member.invited_you {
        tags.push(("🎪", "Invited You"));
    } else if member.sponsored_you {
        tags.push(("🔭", "In Your Invite Chain"));
    }

    if tags.is_empty() {
        return member.nickname.clone();
    }

    let mut html = member.nickname.clone();
    html.push(' ');
    for (icon, tooltip) in &tags {
        html.push_str(&format!(
            "<span class=\"member-icon\" title=\"{}\">{}</span> ",
            tooltip, icon
        ));
    }
    html
}

/// Order member IDs by DFS pre-order traversal of the invite tree.
/// Owner is the root; within siblings, order matches `members.members`
/// (sorted by MemberId after CRDT convergence).
/// Members with broken invite chains are appended at the end.
fn invite_tree_order(owner_id: MemberId, members: &MembersV1) -> Vec<MemberId> {
    let mut children_of: HashMap<MemberId, Vec<MemberId>> = HashMap::new();
    for member in members.members.iter() {
        children_of
            .entry(member.member.invited_by)
            .or_default()
            .push(member.member.id());
    }

    let mut ordered = Vec::new();
    let mut visited = HashSet::new();
    let mut stack = vec![owner_id];
    while let Some(current) = stack.pop() {
        if !visited.insert(current) {
            continue;
        }
        ordered.push(current);
        if let Some(kids) = children_of.get(&current) {
            for &kid in kids.iter().rev() {
                stack.push(kid);
            }
        }
    }

    // Append any members not reachable from the owner (orphaned invite chains)
    for member in members.members.iter() {
        let id = member.member.id();
        if !visited.contains(&id) {
            ordered.push(id);
        }
    }

    ordered
}

#[component]
pub fn MembersView() -> Element {
    let mut invite_modal_active = use_signal(|| false);
    let mut export_modal_active = use_signal(|| false);
    let mut import_modal_active = use_signal(|| false);

    let members = use_memo(move || {
        let board_owner = CURRENT_BOARD.read().owner_key?;

        let boards_read = BOARDS.read();
        let board_data = boards_read.map.get(&board_owner)?;
        let board_state = board_data.board_state.clone();
        let self_member_id: MemberId = board_data.self_sk.verifying_key().into();
        let owner_id: MemberId = board_owner.into();

        let member_info = &board_state.member_info;
        let members = &board_state.members;
        let board_secrets = &board_data.secrets;

        let params = ChatBoardParametersV1 { owner: board_owner };

        let ordered_ids = invite_tree_order(owner_id, members);

        // Build set of admin IDs for quick lookup
        let admin_ids: std::collections::HashSet<MemberId> = board_state
            .admin
            .admins
            .iter()
            .map(|a| a.admin.id())
            .collect();

        // Build display list in tree order
        let mut all_members = Vec::new();
        for &member_id in &ordered_ids {
            let is_owner = member_id == owner_id;
            let is_admin = admin_ids.contains(&member_id);

            let nickname = member_info
                .member_info
                .iter()
                .find(|mi| mi.member_info.member_id == member_id)
                .map(|mi| {
                    match unseal_bytes_with_secrets(
                        &mi.member_info.preferred_nickname,
                        board_secrets,
                    ) {
                        Ok(bytes) => String::from_utf8_lossy(&bytes).to_string(),
                        Err(_) => mi.member_info.preferred_nickname.to_string_lossy(),
                    }
                })
                .unwrap_or_else(|| "Unknown".to_string());

            let member_display = MemberDisplay {
                nickname,
                _member_id: member_id,
                is_owner,
                is_admin,
                is_self: member_id == self_member_id,
                invited_you: members.is_inviter_of(member_id, self_member_id, &params),
                sponsored_you: if is_owner {
                    false
                } else {
                    is_member_sponsor(member_id, members, self_member_id, &params)
                },
                invited_by_you: if is_owner {
                    false
                } else {
                    did_you_invite_member(member_id, members, self_member_id)
                },
                in_your_network: if is_owner {
                    false
                } else {
                    is_in_your_network(member_id, members, self_member_id)
                },
            };

            all_members.push((format_member_display(&member_display), member_id));
        }

        Some(all_members)
    })()
    .unwrap_or_default();

    let handle_member_click = move |member_id| {
        MEMBER_INFO_MODAL.with_mut(|signal| {
            signal.member = Some(member_id);
        });
    };

    // Don't show members panel if no board is selected
    let has_board = CURRENT_BOARD.read().owner_key.is_some();
    if !has_board {
        return rsx! {};
    }

    rsx! {
        div { class: "flex-1 flex flex-col bg-bg overflow-hidden",
            // Header with action buttons
            div { class: "px-4 py-3 border-b border-border flex items-center justify-between flex-shrink-0",
                h2 { class: "text-lg font-semibold text-text flex items-center gap-2",
                    Icon { icon: FaUsers, width: 20, height: 20 }
                    span { "Members" }
                }
                // Action buttons
                div { class: "flex gap-2",
                    button {
                        class: "flex items-center gap-2 px-3 py-2 bg-accent hover:bg-accent-hover text-white text-sm font-medium rounded-lg transition-colors",
                        onclick: move |_| invite_modal_active.set(true),
                        Icon { icon: FaUserPlus, width: 14, height: 14 }
                        span { "Invite" }
                    }
                    button {
                        class: "flex items-center gap-1.5 px-2 py-1.5 bg-surface hover:bg-surface-hover text-text-muted text-xs font-medium rounded-lg transition-colors border border-border",
                        onclick: move |_| export_modal_active.set(true),
                        Icon { icon: FaFileExport, width: 12, height: 12 }
                        span { "Export" }
                    }
                    button {
                        class: "flex items-center gap-1.5 px-2 py-1.5 bg-surface hover:bg-surface-hover text-text-muted text-xs font-medium rounded-lg transition-colors border border-border",
                        onclick: move |_| import_modal_active.set(true),
                        Icon { icon: FaFileImport, width: 12, height: 12 }
                        span { "Import" }
                    }
                }
            }

            // Member list - scrollable
            ul { class: "flex-1 px-4 py-4 space-y-1 overflow-y-auto",
                for (display_name, member_id) in members {
                    li { key: "{member_id}",
                        button {
                            class: "w-full text-left px-4 py-2 rounded-lg text-text hover:bg-surface transition-colors",
                            title: "Member ID: {member_id}",
                            onclick: move |_| handle_member_click(member_id),
                            span {
                                dangerous_inner_html: "{display_name}"
                            }
                        }
                    }
                }
            }
        }
        InviteMemberModal {
            is_active: invite_modal_active
        }
        ExportIdentityModal {
            is_active: export_modal_active
        }
        ImportIdentityModal {
            is_active: import_modal_active
        }
    }
}

#[component]
fn ExportIdentityModal(is_active: Signal<bool>) -> Element {
    let mut token_text = use_signal(String::new);

    // Generate the export token when modal opens
    use_effect(move || {
        if *is_active.read() {
            let board_owner = CURRENT_BOARD.read().owner_key;
            if let Some(owner_key) = board_owner {
                let boards_read = BOARDS.read();
                if let Some(board_data) = boards_read.map.get(&owner_key) {
                    // Get the authorized member, or create one for the owner
                    let authorized_member = if let Some(ref am) = board_data.self_authorized_member
                    {
                        am.clone()
                    } else {
                        // Check if we're the owner - owners can create their own AuthorizedMember
                        let self_vk = board_data.self_sk.verifying_key();
                        if self_vk == owner_key {
                            // Owner is self-invited
                            let owner_id = MemberId::from(&owner_key);
                            let member = Member {
                                owner_member_id: owner_id,
                                invited_by: owner_id,
                                member_vk: owner_key,
                            };
                            AuthorizedMember::new(member, &board_data.self_sk)
                        } else {
                            token_text.set(
                                "Cannot export: membership data not available. \
                                 Try sending a message first."
                                    .to_string(),
                            );
                            return;
                        }
                    };

                    let export = IdentityExport {
                        board_owner: owner_key,
                        signing_key: board_data.self_sk.clone(),
                        authorized_member,
                        invite_chain: board_data.invite_chain.clone(),
                        member_info: board_data.self_member_info.clone(),
                    };
                    token_text.set(export.to_armored_string());
                }
            }
        }
    });

    if !*is_active.read() {
        return rsx! {};
    }

    let handle_copy = move |_| {
        let text = token_text.read().clone();
        crate::util::copy_to_clipboard(&text);
    };

    rsx! {
        div {
            class: "fixed inset-0 bg-black/50 flex items-center justify-center z-50",
            onclick: move |_| {
                is_active.set(false);
                token_text.set(String::new());
            },
            div {
                class: "bg-panel border border-border rounded-xl shadow-lg p-6 max-w-lg w-full mx-4",
                onclick: move |e| e.stop_propagation(),
                h3 { class: "text-lg font-semibold text-text mb-4",
                    "Export Identity"
                }
                p { class: "text-sm text-text-muted mb-3",
                    "Copy this token and import it in another Freetalk client to use the same identity."
                }
                p { class: "text-sm text-yellow-500 font-medium mb-3",
                    "Warning: This token contains your private key. Treat it like a password."
                }
                textarea {
                    class: "w-full h-40 bg-surface border border-border rounded-lg p-3 text-xs font-mono text-text resize-none",
                    readonly: true,
                    value: "{token_text}",
                }
                div { class: "flex justify-end gap-3 mt-4",
                    button {
                        class: "px-4 py-2 bg-surface hover:bg-surface-hover text-text text-sm rounded-lg transition-colors border border-border",
                        onclick: move |_| {
                            is_active.set(false);
                            token_text.set(String::new());
                        },
                        "Close"
                    }
                    button {
                        class: "px-4 py-2 bg-accent hover:bg-accent-hover text-white text-sm font-medium rounded-lg transition-colors",
                        onclick: handle_copy,
                        "Copy to Clipboard"
                    }
                }
            }
        }
    }
}

#[component]
fn ImportIdentityModal(is_active: Signal<bool>) -> Element {
    let mut token_input = use_signal(String::new);
    let mut error_msg = use_signal(|| None::<String>);
    let mut success_msg = use_signal(|| None::<String>);

    if !*is_active.read() {
        return rsx! {};
    }

    let handle_import = move |_| {
        let input = token_input.read().clone();
        match IdentityExport::from_armored_string(&input) {
            Ok(export) => {
                let owner_key = export.board_owner;

                // Check if we already have this board
                {
                    let boards = BOARDS.read();
                    if boards.map.contains_key(&owner_key) {
                        error_msg.set(Some(
                            "You already have an identity for this board.".to_string(),
                        ));
                        return;
                    }
                }

                // Compute contract key from owner key + current WASM
                let contract_key = crate::util::owner_vk_to_contract_key(&owner_key);

                // Create BoardData from the import
                let board_data = crate::board_data::BoardData {
                    owner_vk: owner_key,
                    board_state: Default::default(), // Will be populated on sync
                    self_sk: export.signing_key,
                    contract_key,
                    last_read_message_id: None,
                    secrets: HashMap::new(),
                    current_secret_version: None,
                    last_secret_rotation: None,
                    key_migrated_to_delegate: false,
                    self_authorized_member: Some(export.authorized_member),
                    invite_chain: export.invite_chain,
                    self_member_info: export.member_info,
                };

                // Add to BOARDS and trigger sync
                BOARDS.with_mut(|boards| {
                    boards.map.insert(owner_key, board_data);
                });

                // Set as current board
                CURRENT_BOARD.with_mut(|current| {
                    current.owner_key = Some(owner_key);
                });

                // Trigger a sync for the new board (deferred to avoid RefCell panics)
                mark_needs_sync(owner_key);

                success_msg.set(Some("Identity imported! Syncing board state...".to_string()));
                error_msg.set(None);
            }
            Err(e) => {
                error_msg.set(Some(format!("Invalid token: {}", e)));
                success_msg.set(None);
            }
        }
    };

    rsx! {
        div {
            class: "fixed inset-0 bg-black/50 flex items-center justify-center z-50",
            onclick: move |_| {
                is_active.set(false);
                error_msg.set(None);
                success_msg.set(None);
                token_input.set(String::new());
            },
            div {
                class: "bg-panel border border-border rounded-xl shadow-lg p-6 max-w-lg w-full mx-4",
                onclick: move |e| e.stop_propagation(),
                h3 { class: "text-lg font-semibold text-text mb-4",
                    "Import Identity"
                }
                p { class: "text-sm text-text-muted mb-3",
                    "Paste a Freetalk identity token exported from another client."
                }
                textarea {
                    class: "w-full h-40 bg-surface border border-border rounded-lg p-3 text-xs font-mono text-text resize-none",
                    placeholder: "-----BEGIN FREETALK IDENTITY-----\n...\n-----END FREETALK IDENTITY-----",
                    value: "{token_input}",
                    oninput: move |e| token_input.set(e.value()),
                }
                if let Some(err) = &*error_msg.read() {
                    div { class: "mt-2 text-sm text-red-400",
                        "{err}"
                    }
                }
                if let Some(msg) = &*success_msg.read() {
                    div { class: "mt-2 text-sm text-green-400",
                        "{msg}"
                    }
                }
                div { class: "flex justify-end gap-3 mt-4",
                    button {
                        class: "px-4 py-2 bg-surface hover:bg-surface-hover text-text text-sm rounded-lg transition-colors border border-border",
                        onclick: move |_| {
                            is_active.set(false);
                            error_msg.set(None);
                            success_msg.set(None);
                            token_input.set(String::new());
                        },
                        "Cancel"
                    }
                    button {
                        class: "px-4 py-2 bg-accent hover:bg-accent-hover text-white text-sm font-medium rounded-lg transition-colors",
                        onclick: handle_import,
                        "Import"
                    }
                }
            }
        }
    }
}
