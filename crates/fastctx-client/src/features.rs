//! The interface's optional parts this device shows (`fastctx_types::features`):
//! on a computer, its node keeps them for every interface there, so the
//! graphical interface and the TUI show the same; a device without a node
//! keeps its own.

use fastctx_types::features::{Feature, Features};
use fastctx_types::ipc::NodeFrame;
use fastctx_types::{Call, Reply};

use crate::{CallError, Client, Error, World};

impl World {
    /// The switches as they are; all off until one is set.
    pub async fn features(&self) -> Result<Features, Error> {
        match self.ask(Call::Features).await? {
            Reply::Features(features) => Ok(features),
            other => Err(Error::Unexpected(format!("{other:?}"))),
        }
    }

    /// Turns `feature` on or off; the switches after.
    pub async fn set_feature(&self, feature: Feature, on: bool) -> Result<Features, Error> {
        match self.ask(Call::SetFeature { feature, on }).await? {
            Reply::Features(features) => Ok(features),
            other => Err(Error::Unexpected(format!("{other:?}"))),
        }
    }

    /// Says the interfaces speak the language tagged `language`; the
    /// switches after.
    pub async fn set_language(&self, language: &str) -> Result<Features, Error> {
        match self.ask(Call::SetLanguage { language: language.to_string() }).await? {
            Reply::Features(features) => Ok(features),
            other => Err(Error::Unexpected(format!("{other:?}"))),
        }
    }

    /// Follows the switches: the first `next` gives them as they are, each
    /// later one as they change.
    pub async fn watch_features(&self) -> Result<FeaturesWatch, Error> {
        if let Some(device) = self.device() {
            return Ok(FeaturesWatch::Device {
                device: device.clone(),
                changes: device.feature_changes(),
                first: true,
            });
        }
        let (client, id) = self.connect().await?.watch_features().await?;
        Ok(FeaturesWatch::Local { client, id })
    }
}

/// A watch of the switches; dropping it ends the watch.
pub enum FeaturesWatch {
    Local { client: Client, id: u64 },
    Device { device: std::sync::Arc<crate::device::Device>, changes: tokio::sync::watch::Receiver<u64>, first: bool },
}

impl FeaturesWatch {
    /// The switches, as they are at first, then after each change.
    pub async fn next(&mut self) -> Result<Features, Error> {
        match self {
            FeaturesWatch::Local { client, id } => match client.frame().await? {
                NodeFrame::Features { id: told, features } if told == *id => Ok(features),
                NodeFrame::Reply { id: told, reply } if told == *id => match reply {
                    Ok(_) => Err(CallError::Broken(std::io::Error::other("the features watch ended")).into()),
                    Err(failure) => Err(CallError::Failed(failure).into()),
                },
                _ => {
                    Err(CallError::Broken(std::io::Error::other("the node answered another call on a features watch"))
                        .into())
                }
            },
            FeaturesWatch::Device { device, changes, first } => {
                if !std::mem::take(first) {
                    changes
                        .changed()
                        .await
                        .map_err(|_| CallError::Broken(std::io::Error::other("the device closed")))?;
                }
                changes.borrow_and_update();
                device.features()
            }
        }
    }
}
