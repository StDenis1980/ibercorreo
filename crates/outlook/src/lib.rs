//! Outlook files, L2: read from Microsoft's published Open Specifications, never from Outlook.
//!
//! - [`cfb`]: compound files (MS-CFB), the container of `.msg` files.
//! - [`msg`]: one message saved from Outlook (MS-OXMSG), read into the model.
//! - [`rtf`]: bodies kept as compressed RTF (MS-OXRTFCP), taken apart into HTML or text
//!   (MS-OXRTFEX).

#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

pub mod cfb;
pub mod msg;
pub mod rtf;
#[cfg(any(test, feature = "testing"))]
pub mod testing;
