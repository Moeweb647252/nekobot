macro_rules! string_newtype {
    ($name:ident) => {
        #[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
        pub struct $name(String);

        impl $name {
            pub fn as_str(&self) -> &str {
                &self.0
            }

            pub fn into_string(self) -> String {
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

string_newtype!(ChannelId);
string_newtype!(ChannelName);
string_newtype!(ChatId);
string_newtype!(ChatName);
string_newtype!(SenderId);
string_newtype!(SenderName);
string_newtype!(ReplyTarget);

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
        assert_eq!(reply_target.into_string(), "target-42");
    }
}
