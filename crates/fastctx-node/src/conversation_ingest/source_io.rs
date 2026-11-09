//! Report physical source calls at their actual read boundary, including failure.
//! Every window emits before a later ingest receipt can be observed.
use fastctx_hosts::claude::ingest::{
    checkpoint::Anchor,
    file::{IoCounts, SourceFile, Window},
};
use fastctx_sync::{
    cursor::ByteCount,
    metrics::{
        Cause,
        observe::{Observer, SourceIo},
    },
    store::source_step::SourceStep,
};
use std::{io, path::Path};

pub struct StepObserver<'a> {
    pub observer: Option<&'static Observer>,
    pub step: &'a SourceStep,
}
impl StepObserver<'_> {
    fn emit(&self, cause: Cause, counts: IoCounts) {
        if let Some(observer) = self.observer {
            observer.source_io(&SourceIo {
                source: self.step.primary().source.clone(),
                generation: self.step.generation().clone(),
                operation: Some(self.step.operation().clone()),
                cause,
                open_calls: counts.open_calls,
                read_calls: counts.read_calls,
                logical_read_bytes: counts.read_bytes,
                rpc_calls: 0,
            });
        }
    }
    pub fn open(&self, path: &Path, cause: Cause) -> io::Result<ObservedFile<'_, '_>> {
        match SourceFile::open(path) {
            Ok(mut file) => {
                if self.observer.is_some() {
                    file.observe_io();
                }
                if let Some(counts) = file.take_io() {
                    self.emit(cause, counts);
                }
                Ok(ObservedFile { file, observation: self })
            }
            Err(error) => {
                self.emit(cause, IoCounts { open_calls: 1, ..Default::default() });
                Err(error)
            }
        }
    }
}
pub struct ObservedFile<'a, 'step> {
    pub file: SourceFile,
    observation: &'a StepObserver<'step>,
}
impl ObservedFile<'_, '_> {
    pub fn read_window(&mut self, offset: ByteCount, limit: usize, cause: Cause) -> io::Result<Window> {
        let result = self.file.read_window(offset, limit);
        if let Some(counts) = self.file.take_io() {
            self.observation.emit(cause, counts);
        }
        result
    }
    pub fn check_anchor(&mut self, anchor: &Anchor, cause: Cause) -> io::Result<bool> {
        let result = self.file.check_anchor(anchor);
        if let Some(counts) = self.file.take_io() {
            self.observation.emit(cause, counts);
        }
        result
    }
}
