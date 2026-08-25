use crate::SessionRecord;
use splitdesk_core::{CreateSessionRequest, Error, SessionId};

pub trait SessionBackend: Send + Sync {
    fn create_session(&self, req: CreateSessionRequest) -> Result<SessionRecord, Error>;
    fn destroy_session(&self, id: &SessionId) -> Result<(), Error>;
    fn list_sessions(&self) -> Result<Vec<SessionRecord>, Error>;
    fn info(&self, id: &SessionId) -> Result<SessionRecord, Error>;
}
