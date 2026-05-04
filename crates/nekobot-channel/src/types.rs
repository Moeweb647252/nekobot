//! String newtype wrappers for channel domain identifiers.
//!
//! Each type wraps a plain `String` to prevent accidental mixing of IDs,
//! names, and targets across different concepts.

macro_rules! string_newtype {
    ($name:ident, $doc:literal) => {
        #[doc = $doc]
        #[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
        pub struct $name(String);

        impl $name {
            pub fn as_str(&self) -> &str {
                &self.0
            }

            pub fn into_inner(self) -> String {
                self.0
            }
        }

        impl From<String> for $name {
            fn from(value: String) -> Self {
                Self(value)
            }
        }

        impl From<&str> for $name {
            fn from(value: &str) -> Self {
                Self(value.to_owned())
            }
        }

        impl AsRef<str> for $name {
            fn as_ref(&self) -> &str {
                self.as_str()
            }
        }

        impl std::fmt::Display for $name {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str(self.as_str())
            }
        }
    };
}

string_newtype!(ChannelId, "Unique identifier for a channel instance.");
string_newtype!(ChannelName, "Human-readable name of a channel.");
string_newtype!(
    ChatId,
    "Unique identifier for a conversation within a channel."
);

impl ChatId {
    /// Returns true if this is a C2C (private) chat.
    pub fn is_c2c(&self) -> bool {
        self.0.starts_with("c2c:")
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChatType {
    Private,
    Group,
}

impl ChatType {
    pub fn is_private(self) -> bool { matches!(self, ChatType::Private) }
}

string_newtype!(ChatName, "Human-readable name of a conversation.");
string_newtype!(SenderId, "Unique identifier for a message sender.");
string_newtype!(SenderName, "Human-readable name of a message sender.");
string_newtype!(
    ReplyTarget,
    "Opaque routing token used to direct replies back to the correct conversation."
);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn newtypes_preserve_inner_values() {
        let channel_id = ChannelId::from("qq-main");
        let chat_id = ChatId::from("group-42".to_owned());
        let reply_target = ReplyTarget::from("target-42");

        assert_eq!(channel_id.as_str(), "qq-main");
        assert_eq!(chat_id.as_str(), "group-42");
        assert_eq!(reply_target.into_inner(), "target-42");
    }
}
