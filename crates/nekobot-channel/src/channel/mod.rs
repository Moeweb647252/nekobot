//! Concrete channel adapter implementations.

mod qq;
mod weixin;

pub use qq::QQChannel;
pub use weixin::WeiXinChannel;
