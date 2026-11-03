//! Outlook files, L2: read from Microsoft's published Open Specifications, never from Outlook.
//!
//! - [`cfb`]: compound files (MS-CFB), the container of `.msg` files.
//! - [`msg`]: one message saved from Outlook (MS-OXMSG), read into the model.
//! - `props`: the message properties both share, turned into the model.
//! - [`pst`]: Outlook data files (MS-PST): folders and the mail, appointments and contacts in
//!   them, read through the node database (`ndb`) and the heap, table and property layer (`ltp`);
//!   `named` resolves named properties, `items` turns appointments (MS-OXOCAL, series from
//!   `recur`) and contacts (MS-OXOCNTC) into the model.
//! - [`rtf`]: bodies kept as compressed RTF (MS-OXRTFCP), taken apart into HTML or text
//!   (MS-OXRTFEX).

#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

pub mod cfb;
mod items;
mod ltp;
pub mod msg;
mod named;
mod ndb;
mod props;
pub mod pst;
mod recur;
pub mod rtf;
#[cfg(any(test, feature = "testing"))]
pub mod testing;
