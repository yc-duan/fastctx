//! Independently invalidated context sources, in their refresh order.

#[derive(Clone, Copy, Debug)]
#[repr(usize)]
pub(crate) enum Part {
    Focus,
    User,
    Project,
    Guide,
    Plan,
    Skills,
    History,
}

pub(super) const COUNT: usize = 7;
pub(super) const ALL: [Part; COUNT] =
    [Part::Focus, Part::User, Part::Project, Part::Guide, Part::Plan, Part::Skills, Part::History];

#[derive(Clone, Copy, Default)]
pub(super) struct Parts(u8);

impl Parts {
    pub(super) fn of(parts: &[Part]) -> Self {
        Self(parts.iter().fold(0, |mask, part| mask | (1 << *part as usize)))
    }
    pub(super) fn contains(self, part: Part) -> bool {
        self.0 & (1 << part as usize) != 0
    }
    pub(super) fn changed(before: &[u64; COUNT], after: &[u64; COUNT]) -> Self {
        Self(
            ALL.iter()
                .filter(|part| before[**part as usize] != after[**part as usize])
                .fold(0, |mask, part| mask | (1 << *part as usize)),
        )
    }
    pub(super) fn empty(self) -> bool {
        self.0 == 0
    }
}

impl Part {
    pub(super) fn record(self) {
        static COUNTS: std::sync::LazyLock<[&'static fastctx_platform::readings::Counter; COUNT]> =
            std::sync::LazyLock::new(|| {
                [
                    fastctx_platform::readings::counter("context.refresh.focus"),
                    fastctx_platform::readings::counter("context.refresh.user"),
                    fastctx_platform::readings::counter("context.refresh.project"),
                    fastctx_platform::readings::counter("context.refresh.guide"),
                    fastctx_platform::readings::counter("context.refresh.plan"),
                    fastctx_platform::readings::counter("context.refresh.skills"),
                    fastctx_platform::readings::counter("context.refresh.history"),
                ]
            });
        COUNTS[self as usize].add(1);
    }
}
