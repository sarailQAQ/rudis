mod get;
pub use get::Get;

mod publish;
pub use publish::Publish;

mod set;
pub use set::Set;

mod subscribe;
pub use subscribe::{Subscribe, Unsubscribe};

mod unknown;
pub use unknown::Unknown;
use crate::db::Db;

use crate::{Connection, Frame};
use crate::parse::Parse;
use crate::shutdown::Shutdown;


/// Enumeration of supported Redis commands.
///
/// Methods called on `Command` are delegated to the command implementation.
#[derive(Debug)]
pub enum Command {
    Get(Get),
    Publish(Publish),
    Set(Set),
    Subscribe(Subscribe),
    Unsubscribe(Unsubscribe),
    Unknown(Unknown),
}

impl Command {
    pub fn from_frame(frame: Frame) -> crate::Result<Command> {
        let mut parse = Parse::new(frame)?;
        let cmd_name = parse.next_string()?;

        let command = match &cmd_name[..] {
            "get" => Command::Get(Get::parse_frames(&mut parse)?),
            "set" => Command::Set(Set::parse_frames(&mut parse)?),
            _ => {
                return Ok(Command::Unknown(Unknown::new(cmd_name)));
            }
        };

        parse.finish()?;

        Ok(command)
    }

    pub(crate) async fn apply(
        self,
        db: &Db,
        dst: &mut Connection,
        shutdown: &mut Shutdown,
    ) -> crate::Result<()> {
        use Command::*;

        match self {
            Get(cmd) => cmd.apply(db, dst).await,
            Set(cmd) => cmd.apply(db, dst).await,
            Unknown(cmd) => cmd.apply(dst).await,
            // Subscribe(cmd) => cmd.apply(db, dst, shutdown).await,
            // Publish(cmd) => cmd.apply(db, dst).await,
            // `Unsubscribe` cannot be applied. It may only be received from the
            // context of a `Subscribe` command.
            _  => Err("Unimplemented command.".into()),
        }
    }

    pub(crate) fn get_name(&self) -> &str {
        match self {
            Command::Get(_) => "get",
            Command::Publish(_) => "pub",
            Command::Set(_) => "set",
            Command::Subscribe(_) => "subscribe",
            Command::Unsubscribe(_) => "unsubscribe",
            Command::Unknown(cmd) => cmd.get_name(),
        }
    }
}