//! A receipt may belong to an agent assisting only one of this device's accounts.

use crate::Error;
use fastctx_types::access::Purpose;
use fastctx_types::attachments::AttachmentSource;
use fastctx_types::remote::{RemoteCall, RemoteReply};
use fastctx_types::thumbnail::Thumbnail;

impl super::Device {
    pub(crate) async fn thumbnail_attachment(
        &self,
        attachment: AttachmentSource,
        max: u32,
    ) -> Result<Thumbnail, Error> {
        let machine = attachment.thumbnail_owner().machine.clone();
        let session = self.reaching(&machine.clone().into())?;
        session.account_for(&machine, Purpose::Agents)?;
        let accounts = {
            let view = session.link.view();
            let me = session.link.membership().name.clone();
            view.me()
                .into_iter()
                .flat_map(|member| &member.logins)
                .filter(|account| view.admits(&me, account, &machine, Purpose::Agents).is_ok())
                .cloned()
                .collect::<Vec<_>>()
        };
        let mut refused = None;
        for account in accounts {
            match fastctx_net::cancellable::call(
                &session.link,
                &machine,
                &account,
                RemoteCall::AttachmentThumbnail { attachment: attachment.clone(), max },
            )
            .await
            .map_err(super::failed)?
            {
                Err(error) if error.not_run => refused = Some(error),
                Err(error) => return Err(crate::CallError::Failed(error).into()),
                Ok(RemoteReply::Thumbnail(thumbnail)) => return Ok(thumbnail),
                Ok(_) => return Err(super::failed("expected an attachment thumbnail")),
            }
        }
        Err(super::failed(refused.map_or_else(
            || "this device's accounts changed before reading the attachment".into(),
            |error| error.to_string(),
        )))
    }
}
