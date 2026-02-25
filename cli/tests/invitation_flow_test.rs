use anyhow::{anyhow, Result};
use ed25519_dalek::SigningKey;
use freenet_scaffold::ComposableState;
use river_core::board_state::configuration::{AuthorizedConfigurationV1, Configuration};
use river_core::board_state::member::{AuthorizedMember, Member};
use river_core::board_state::member_info::{AuthorizedMemberInfo, MemberInfo};
use river_core::board_state::message::BoardMessageBody;
use river_core::board_state::privacy::{BoardDisplayMetadata, SealedBytes};
use river_core::board_state::{ChatBoardParametersV1, ChatBoardStateV1};

#[test]
fn test_invitation_acceptance_initializes_board_state_correctly() -> Result<()> {
    // Create owner signing key
    let owner_sk = SigningKey::from_bytes(&rand::Rng::gen::<[u8; 32]>(&mut rand::thread_rng()));
    let owner_vk = owner_sk.verifying_key();

    // Create a board state as it would exist on the network
    let mut board_state = ChatBoardStateV1::default();

    // Set up proper configuration
    let config = Configuration {
        display: BoardDisplayMetadata {
            name: SealedBytes::public("Test Board".to_string().into_bytes()),
            description: None,
        },
        owner_member_id: owner_vk.into(),
        ..Default::default()
    };
    board_state.configuration = AuthorizedConfigurationV1::new(config, &owner_sk);

    // Add owner's member info
    let owner_info = MemberInfo {
        member_id: owner_vk.into(),
        version: 0,
        preferred_nickname: SealedBytes::public("Owner".to_string().into_bytes()),
    };
    let auth_owner_info = AuthorizedMemberInfo::new_with_member_key(owner_info, &owner_sk);
    board_state.member_info.member_info.push(auth_owner_info);

    // Create invitee
    let invitee_sk = SigningKey::from_bytes(&rand::Rng::gen::<[u8; 32]>(&mut rand::thread_rng()));
    let invitee_vk = invitee_sk.verifying_key();

    // Create invitation member entry
    let member = Member {
        owner_member_id: owner_vk.into(),
        member_vk: invitee_vk,
        invited_by: owner_vk.into(),
    };
    let authorized_member = AuthorizedMember::new(member, &owner_sk);

    // Simulate what happens in accept_invitation:
    // 1. Apply member delta
    let parameters = ChatBoardParametersV1 { owner: owner_vk };
    let members_delta =
        river_core::board_state::member::MembersDelta::new(vec![authorized_member.clone()]);
    board_state
        .members
        .apply_delta(&board_state.clone(), &parameters, &Some(members_delta))
        .map_err(|e| anyhow!("Failed to apply member delta: {}", e))?;

    // 2. Add member info
    let member_info = MemberInfo {
        member_id: invitee_vk.into(),
        version: 0,
        preferred_nickname: SealedBytes::public("User2".to_string().into_bytes()),
    };
    let auth_member_info = AuthorizedMemberInfo::new_with_member_key(member_info, &invitee_sk);
    board_state.member_info.member_info.push(auth_member_info);

    // Validate the state is properly initialized
    assert_ne!(
        board_state.configuration.configuration.owner_member_id,
        river_core::board_state::member::MemberId(freenet_scaffold::util::FastHash(0)),
        "owner_member_id should not be default"
    );

    assert_eq!(
        board_state.configuration.configuration.owner_member_id,
        owner_vk.into(),
        "owner_member_id should match the board owner"
    );

    assert_eq!(
        board_state.members.members.len(),
        1,
        "Should have one member (the invitee)"
    );

    assert_eq!(
        board_state.members.members[0].member.member_vk, invitee_vk,
        "Member should be the invitee"
    );

    assert_eq!(
        board_state.member_info.member_info.len(),
        2,
        "Should have member info for owner and invitee"
    );

    // Verify the board state is valid
    board_state
        .verify(&board_state, &parameters)
        .map_err(|e| anyhow!("Board state verification failed: {}", e))?;

    Ok(())
}

#[test]
fn test_message_validation_after_invitation_acceptance() -> Result<()> {
    use river_core::board_state::message::{AuthorizedMessageV1, MessageV1};
    use std::time::SystemTime;

    // Set up board with owner and invited member
    let owner_sk = SigningKey::from_bytes(&rand::Rng::gen::<[u8; 32]>(&mut rand::thread_rng()));
    let owner_vk = owner_sk.verifying_key();

    let invitee_sk = SigningKey::from_bytes(&rand::Rng::gen::<[u8; 32]>(&mut rand::thread_rng()));
    let invitee_vk = invitee_sk.verifying_key();

    let mut board_state = ChatBoardStateV1::default();

    // Configure board
    let config = Configuration {
        display: BoardDisplayMetadata {
            name: SealedBytes::public("Test Board".to_string().into_bytes()),
            description: None,
        },
        owner_member_id: owner_vk.into(),
        ..Default::default()
    };
    board_state.configuration = AuthorizedConfigurationV1::new(config, &owner_sk);

    // Add invitee to members
    let member = Member {
        owner_member_id: owner_vk.into(),
        member_vk: invitee_vk,
        invited_by: owner_vk.into(),
    };
    let authorized_member = AuthorizedMember::new(member, &owner_sk);
    board_state.members.members.push(authorized_member);

    // Create a message from the invitee
    let message = MessageV1 {
        board_owner: owner_vk.into(),
        author: invitee_vk.into(),
        content: BoardMessageBody::public("Hello from invited user!".to_string()),
        time: SystemTime::now(),
    };
    let auth_message = AuthorizedMessageV1::new(message, &invitee_sk);

    // Apply message via delta
    let parameters = ChatBoardParametersV1 { owner: owner_vk };
    let message_delta = vec![auth_message.clone()];

    board_state
        .recent_messages
        .apply_delta(&board_state.clone(), &parameters, &Some(message_delta))
        .map_err(|e| anyhow!("Failed to apply message delta: {}", e))?;

    // Verify the message was retained (not filtered out)
    assert_eq!(
        board_state.recent_messages.messages.len(),
        1,
        "Message from invited member should be retained"
    );

    assert_eq!(
        board_state.recent_messages.messages[0].message.content,
        BoardMessageBody::public("Hello from invited user!".to_string()),
        "Message content should match"
    );

    // Verify the entire state is valid
    board_state
        .verify(&board_state, &parameters)
        .map_err(|e| anyhow!("Board state verification failed: {}", e))?;

    Ok(())
}

#[test]
fn test_uninvited_user_messages_are_filtered() -> Result<()> {
    use river_core::board_state::message::{AuthorizedMessageV1, MessageV1};
    use std::time::SystemTime;

    // Set up board with owner only
    let owner_sk = SigningKey::from_bytes(&rand::Rng::gen::<[u8; 32]>(&mut rand::thread_rng()));
    let owner_vk = owner_sk.verifying_key();

    let uninvited_sk = SigningKey::from_bytes(&rand::Rng::gen::<[u8; 32]>(&mut rand::thread_rng()));
    let uninvited_vk = uninvited_sk.verifying_key();

    let mut board_state = ChatBoardStateV1::default();

    // Configure board
    let config = Configuration {
        display: BoardDisplayMetadata {
            name: SealedBytes::public("Test Board".to_string().into_bytes()),
            description: None,
        },
        owner_member_id: owner_vk.into(),
        ..Default::default()
    };
    board_state.configuration = AuthorizedConfigurationV1::new(config, &owner_sk);

    // Do NOT add uninvited user to members

    // Create a message from the uninvited user
    let message = MessageV1 {
        board_owner: owner_vk.into(),
        author: uninvited_vk.into(),
        content: BoardMessageBody::public("Hello from uninvited user!".to_string()),
        time: SystemTime::now(),
    };
    let auth_message = AuthorizedMessageV1::new(message, &uninvited_sk);

    // Apply message via delta
    let parameters = ChatBoardParametersV1 { owner: owner_vk };
    let message_delta = vec![auth_message];

    board_state
        .recent_messages
        .apply_delta(&board_state.clone(), &parameters, &Some(message_delta))
        .map_err(|e| anyhow!("Failed to apply message delta: {}", e))?;

    // Verify the message was filtered out
    assert_eq!(
        board_state.recent_messages.messages.len(),
        0,
        "Message from uninvited member should be filtered out"
    );

    Ok(())
}
