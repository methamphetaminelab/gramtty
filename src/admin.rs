use tdlib_rs::enums::{self, ChatEventAction as A, ChatMemberStatus};
use tdlib_rs::types;

use crate::format;
use crate::model::Sender;

pub const LOG_PAGE: usize = 100;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum MemberFilter {
    #[default]
    Recent,
    Admins,
    Restricted,
    Banned,
    Bots,
}

impl MemberFilter {
    pub const ALL: [MemberFilter; 5] = [
        MemberFilter::Recent,
        MemberFilter::Admins,
        MemberFilter::Restricted,
        MemberFilter::Banned,
        MemberFilter::Bots,
    ];

    pub fn name(self) -> &'static str {
        match self {
            MemberFilter::Recent => "recent",
            MemberFilter::Admins => "admins",
            MemberFilter::Restricted => "restricted",
            MemberFilter::Banned => "banned",
            MemberFilter::Bots => "bots",
        }
    }

    pub fn step(self, forward: bool) -> MemberFilter {
        crate::config::cycle(&Self::ALL, self, forward)
    }

    pub fn to_td(self, query: &str) -> enums::SupergroupMembersFilter {
        let query = query.trim().to_owned();
        match self {
            MemberFilter::Restricted => enums::SupergroupMembersFilter::Restricted(
                types::SupergroupMembersFilterRestricted { query },
            ),
            MemberFilter::Banned => {
                enums::SupergroupMembersFilter::Banned(types::SupergroupMembersFilterBanned {
                    query,
                })
            }
            _ if !query.is_empty() => {
                enums::SupergroupMembersFilter::Search(types::SupergroupMembersFilterSearch {
                    query,
                })
            }
            MemberFilter::Recent => enums::SupergroupMembersFilter::Recent,
            MemberFilter::Admins => enums::SupergroupMembersFilter::Administrators,
            MemberFilter::Bots => enums::SupergroupMembersFilter::Bots,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Member {
    pub sender: Sender,
    pub status: String,
    pub joined: i32,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct MemberPage {
    pub total: i32,
    pub members: Vec<Member>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LogEntry {
    pub id: i64,
    pub date: i32,
    pub actor: Sender,
    pub action: String,
    pub target: Option<Sender>,
    pub detail: String,
}

pub fn status_label(s: &ChatMemberStatus) -> String {
    let titled = |base: &str, title: &str| {
        if title.is_empty() {
            base.to_owned()
        } else {
            format!("{base} · {title}")
        }
    };
    match s {
        ChatMemberStatus::Creator(c) => titled("owner", &c.custom_title),
        ChatMemberStatus::Administrator(a) => titled("admin", &a.custom_title),
        ChatMemberStatus::Member(_) => "member".to_owned(),
        ChatMemberStatus::Restricted(r) => match r.restricted_until_date {
            0 => "restricted".to_owned(),
            d => format!("restricted until {}", format::stamp(i64::from(d))),
        },
        ChatMemberStatus::Left => "left".to_owned(),
        ChatMemberStatus::Banned(b) => match b.banned_until_date {
            0 => "banned".to_owned(),
            d => format!("banned until {}", format::stamp(i64::from(d))),
        },
    }
}

pub fn convert_member(m: types::ChatMember) -> Member {
    Member {
        sender: crate::td::convert_sender(&m.member_id),
        status: status_label(&m.status),
        joined: m.joined_chat_date,
    }
}

fn message_preview(m: &types::Message) -> String {
    let msg = crate::td::convert_message(m.clone());
    let text = msg.body.text.replace('\n', " ");
    if !text.trim().is_empty() {
        return text;
    }
    msg.attachments.first().map_or_else(String::new, |a| {
        format!("[{}]", crate::details::kind_name(a.kind))
    })
}

fn toggled(on: bool, what: &str) -> String {
    format!("{} {what}", if on { "enabled" } else { "disabled" })
}

fn change(old: &str, new: &str) -> String {
    format!("\"{old}\" → \"{new}\"")
}

fn fallback_name(action: &A) -> String {
    let kind = serde_json::to_value(action)
        .ok()
        .and_then(|v| v["@type"].as_str().map(str::to_owned))
        .unwrap_or_default();
    let name = kind.strip_prefix("chatEvent").unwrap_or(&kind);
    crate::td::split_camel(name).to_lowercase()
}

pub fn describe(action: &A) -> (String, Option<Sender>, String) {
    let user = |id: i64| Some(Sender::User(id));
    let (what, target, detail): (&str, Option<Sender>, String) = match action {
        A::ChatEventMessageEdited(e) => ("edited a message", None, message_preview(&e.new_message)),
        A::ChatEventMessageDeleted(e) => ("deleted a message", None, message_preview(&e.message)),
        A::ChatEventMessagePinned(e) => ("pinned a message", None, message_preview(&e.message)),
        A::ChatEventMessageUnpinned(e) => ("unpinned a message", None, message_preview(&e.message)),
        A::ChatEventPollStopped(e) => ("stopped a poll", None, message_preview(&e.message)),
        A::ChatEventMemberJoined => ("joined", None, String::new()),
        A::ChatEventMemberJoinedByInviteLink(e) => (
            "joined by invite link",
            None,
            e.invite_link.invite_link.clone(),
        ),
        A::ChatEventMemberJoinedByRequest(e) => (
            "joined, approved by",
            user(e.approver_user_id),
            String::new(),
        ),
        A::ChatEventMemberInvited(e) => ("invited", user(e.user_id), String::new()),
        A::ChatEventMemberLeft => ("left", None, String::new()),
        A::ChatEventMemberPromoted(e) => (
            "changed rights of",
            user(e.user_id),
            format!(
                "{} → {}",
                status_label(&e.old_status),
                status_label(&e.new_status)
            ),
        ),
        A::ChatEventMemberRestricted(e) => (
            "changed restrictions of",
            Some(crate::td::convert_sender(&e.member_id)),
            format!(
                "{} → {}",
                status_label(&e.old_status),
                status_label(&e.new_status)
            ),
        ),
        A::ChatEventTitleChanged(e) => (
            "changed the title",
            None,
            change(&e.old_title, &e.new_title),
        ),
        A::ChatEventDescriptionChanged(e) => (
            "changed the description",
            None,
            change(&e.old_description, &e.new_description),
        ),
        A::ChatEventUsernameChanged(e) => (
            "changed the username",
            None,
            change(&e.old_username, &e.new_username),
        ),
        A::ChatEventSlowModeDelayChanged(e) => (
            "changed slow mode",
            None,
            format!(
                "{} → {}",
                format::ttl(e.old_slow_mode_delay),
                format::ttl(e.new_slow_mode_delay)
            ),
        ),
        A::ChatEventMessageAutoDeleteTimeChanged(e) => (
            "changed the auto-delete timer",
            None,
            format!(
                "{} → {}",
                format::ttl(e.old_message_auto_delete_time),
                format::ttl(e.new_message_auto_delete_time)
            ),
        ),
        A::ChatEventPermissionsChanged(_) => ("changed default permissions", None, String::new()),
        A::ChatEventPhotoChanged(_) => ("changed the photo", None, String::new()),
        A::ChatEventHasProtectedContentToggled(e) => (
            "",
            None,
            toggled(e.has_protected_content, "content protection"),
        ),
        A::ChatEventInvitesToggled(e) => {
            ("", None, toggled(e.can_invite_users, "invites by members"))
        }
        A::ChatEventIsAllHistoryAvailableToggled(e) => (
            "",
            None,
            toggled(e.is_all_history_available, "history for new members"),
        ),
        A::ChatEventSignMessagesToggled(e) => ("", None, toggled(e.sign_messages, "signatures")),
        A::ChatEventIsForumToggled(e) => ("", None, toggled(e.is_forum, "topics")),
        A::ChatEventForumTopicCreated(e) => ("created topic", None, e.topic_info.name.clone()),
        A::ChatEventForumTopicDeleted(e) => ("deleted topic", None, e.topic_info.name.clone()),
        A::ChatEventInviteLinkRevoked(e) => (
            "revoked invite link",
            None,
            e.invite_link.invite_link.clone(),
        ),
        A::ChatEventInviteLinkDeleted(e) => (
            "deleted invite link",
            None,
            e.invite_link.invite_link.clone(),
        ),
        other => return (fallback_name(other), None, String::new()),
    };
    (what.to_owned(), target, detail)
}

pub fn convert_event(e: types::ChatEvent) -> LogEntry {
    let (action, target, detail) = describe(&e.action);
    LogEntry {
        id: e.id,
        date: e.date,
        actor: crate::td::convert_sender(&e.member_id),
        action,
        target,
        detail,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn action(v: serde_json::Value) -> A {
        serde_json::from_value(v).unwrap()
    }

    #[test]
    fn filters_map_to_tdlib_with_search() {
        assert_eq!(
            MemberFilter::Recent.to_td(""),
            enums::SupergroupMembersFilter::Recent
        );
        assert_eq!(
            MemberFilter::Admins.to_td(""),
            enums::SupergroupMembersFilter::Administrators
        );
        assert_eq!(
            MemberFilter::Bots.to_td(""),
            enums::SupergroupMembersFilter::Bots
        );
        assert_eq!(
            MemberFilter::Recent.to_td(" ann "),
            enums::SupergroupMembersFilter::Search(types::SupergroupMembersFilterSearch {
                query: "ann".to_owned()
            })
        );
        assert_eq!(
            MemberFilter::Banned.to_td("bob"),
            enums::SupergroupMembersFilter::Banned(types::SupergroupMembersFilterBanned {
                query: "bob".to_owned()
            })
        );
        assert_eq!(MemberFilter::Bots.step(true), MemberFilter::Recent);
        assert_eq!(MemberFilter::Recent.step(false), MemberFilter::Bots);
    }

    #[test]
    fn statuses_read_like_labels() {
        let s = ChatMemberStatus::Administrator(types::ChatMemberStatusAdministrator {
            custom_title: "mod".to_owned(),
            can_be_edited: false,
            rights: Default::default(),
        });
        assert_eq!(status_label(&s), "admin · mod");
        assert_eq!(status_label(&ChatMemberStatus::Left), "left");
        let banned: ChatMemberStatus = serde_json::from_value(
            serde_json::json!({"@type": "chatMemberStatusBanned", "banned_until_date": 0}),
        )
        .unwrap();
        assert_eq!(status_label(&banned), "banned");
    }

    #[test]
    fn events_are_described_with_target_and_detail() {
        let (what, target, detail) = describe(&action(serde_json::json!({
            "@type": "chatEventTitleChanged",
            "old_title": "Old",
            "new_title": "New"
        })));
        assert_eq!(what, "changed the title");
        assert_eq!(target, None);
        assert_eq!(detail, "\"Old\" → \"New\"");
        let (what, target, _) = describe(&action(serde_json::json!({
            "@type": "chatEventMemberInvited",
            "user_id": 42,
            "status": {"@type": "chatMemberStatusMember", "member_until_date": 0}
        })));
        assert_eq!(what, "invited");
        assert_eq!(target, Some(Sender::User(42)));
        let (_, _, detail) = describe(&action(serde_json::json!({
            "@type": "chatEventInvitesToggled",
            "can_invite_users": false
        })));
        assert_eq!(detail, "disabled invites by members");
        let (what, _, _) = describe(&A::ChatEventMemberLeft);
        assert_eq!(what, "left");
        let (what, _, _) = describe(&action(serde_json::json!({
            "@type": "chatEventVideoChatMuteNewParticipantsToggled",
            "mute_new_participants": true
        })));
        assert_eq!(what, "video chat mute new participants toggled");
    }

    #[test]
    fn events_convert_with_actor() {
        let e: types::ChatEvent = serde_json::from_value(serde_json::json!({
            "id": "77",
            "date": 1_700_000_000,
            "member_id": {"@type": "messageSenderUser", "user_id": 5},
            "action": {"@type": "chatEventMemberJoined"}
        }))
        .unwrap();
        let entry = convert_event(e);
        assert_eq!(entry.id, 77);
        assert_eq!(entry.actor, Sender::User(5));
        assert_eq!(entry.action, "joined");
    }
}
