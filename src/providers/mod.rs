#![allow(dead_code)]

mod openai;
pub use self::SendSyncTextProvider as TextProvider;
pub use _dynosaur_macro__dynimageprovider::_DynImageProvider as DynImageProvider;
pub use _dynosaur_macro__dyntextprovider::_DynTextProvider as DynTextProvider;
pub use openai::OpenAI;

pub enum Message {
  User(String),
  Assitant(String),
  System(String),
}

#[trait_variant::make(SendSyncTextProvider: Send + Sync)]
#[dynosaur::dynosaur(_DynTextProvider = dyn SendSyncTextProvider)]
pub trait _TextProvider {
  async fn completion(&self, msg: Vec<Message>) -> anyhow::Result<String>;
}

#[dynosaur::dynosaur(_DynImageProvider)]
pub trait ImageProvider {
  async fn explain(&self, img: Vec<u8>) -> anyhow::Result<String>;
}
