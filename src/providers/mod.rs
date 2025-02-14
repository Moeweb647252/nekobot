#![allow(dead_code)]

mod openai;
pub use self::SendSyncImageToTextProvider as ImageToTextProvider;
pub use self::SendSyncTextToImageProvider as TextToImageProvider;
pub use self::SendSyncTextToTextProvider as TextToTextProvider;
pub use _dynosaur_macro__dynimagetotextprovider::_DynImageToTextProvider as DynImageToTextProvider;
pub use _dynosaur_macro__dyntexttoimageprovider::_DynTextToImageProvider as DynTextToImageProvider;
pub use _dynosaur_macro__dyntexttotextprovider::_DynTextToTextProvider as DynTextToTextProvider;
pub use openai::OpenAI;

pub enum Message {
  User(String),
  Assitant(String),
  System(String),
}

pub struct Usage {
  pub completion: usize,
  pub prompt: usize,
}

#[trait_variant::make(SendSyncTextToTextProvider: Send + Sync)]
#[dynosaur::dynosaur(_DynTextToTextProvider = dyn SendSyncTextToTextProvider)]
pub trait _TextToTextProvider {
  async fn completion(&self, msg: Vec<Message>) -> anyhow::Result<(String, Option<Usage>)>;
}

#[trait_variant::make(SendSyncImageToTextProvider: Send + Sync)]
#[dynosaur::dynosaur(_DynImageToTextProvider = dyn SendSyncImageToTextProvider)]
pub trait _ImageToTextProvider {
  async fn explain(&self, img: Vec<u8>) -> anyhow::Result<String>;
}

#[trait_variant::make(SendSyncTextToImageProvider: Send + Sync)]
#[dynosaur::dynosaur(_DynTextToImageProvider = dyn SendSyncTextToImageProvider)]
pub trait _TextToImageProvider {
  async fn generate(&self, msg: String) -> anyhow::Result<Vec<u8>>;
}
