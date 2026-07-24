//! Localized copy for the one-time output-budget migration notice.

use crate::control::i18n::Language;

#[derive(Debug)]
pub(crate) struct MigrationMessages {
    pub(crate) title: &'static str,
    pub(crate) body: &'static str,
    pub(crate) action_confirm: &'static str,
    pub(crate) footer_confirm: &'static str,
}

macro_rules! migration_messages {
    ($title:expr, $body:expr, $action_confirm:expr, $footer_confirm:expr $(,)?) => {
        MigrationMessages {
            title: $title,
            body: $body,
            action_confirm: $action_confirm,
            footer_confirm: $footer_confirm,
        }
    };
}

const EN: MigrationMessages = migration_messages!(
    "Output budgets updated",
    "FastCtx {version} recentered the recommended per-tool output budgets. Your settings have been updated to the new defaults. Re-apply to write them into Codex.",
    "OK",
    "Enter / Esc  Confirm",
);
const ZH_CN: MigrationMessages = migration_messages!(
    "输出档位已更新",
    "FastCtx {version} 重新调整了各工具输出档位的推荐默认值，已更新到你的配置。重新 Apply 即可写入 Codex 生效。",
    "确定",
    "Enter / Esc  确定",
);
const ZH_TW: MigrationMessages = migration_messages!(
    "輸出預算已更新",
    "FastCtx {version} 重新調整了各工具輸出預算的建議預設值，並已更新你的設定。重新 Apply 即可寫入 Codex 生效。",
    "確定",
    "Enter / Esc  確定",
);
const JA: MigrationMessages = migration_messages!(
    "出力予算を更新しました",
    "FastCtx {version} でツールごとの推奨出力予算を再調整し、設定を新しい既定値に更新しました。もう一度 Apply すると Codex に反映されます。",
    "確認",
    "Enter / Esc  確認",
);
const KO: MigrationMessages = migration_messages!(
    "출력 예산이 업데이트되었습니다",
    "FastCtx {version}에서 도구별 권장 출력 예산을 다시 조정하고 설정을 새 기본값으로 업데이트했습니다. 다시 Apply하면 Codex에 반영됩니다.",
    "확인",
    "Enter / Esc  확인",
);
const ES: MigrationMessages = migration_messages!(
    "Presupuestos de salida actualizados",
    "FastCtx {version} reajustó los presupuestos de salida recomendados por herramienta. Tu configuración se actualizó con los nuevos valores predeterminados. Vuelve a aplicar para escribirlos en Codex.",
    "Aceptar",
    "Enter / Esc  Aceptar",
);
const FR: MigrationMessages = migration_messages!(
    "Budgets de sortie mis à jour",
    "FastCtx {version} a recentré les budgets de sortie recommandés par outil. Vos réglages ont été remplacés par les nouvelles valeurs par défaut. Relancez Apply pour les écrire dans Codex.",
    "Valider",
    "Enter / Esc  Valider",
);
const DE: MigrationMessages = migration_messages!(
    "Ausgabebudgets aktualisiert",
    "FastCtx {version} hat die empfohlenen Ausgabebudgets pro Werkzeug neu ausgerichtet. Ihre Einstellungen wurden auf die neuen Standardwerte gesetzt. Führen Sie Apply erneut aus, um sie in Codex zu schreiben.",
    "Bestätigen",
    "Enter / Esc  Bestätigen",
);
const PT_BR: MigrationMessages = migration_messages!(
    "Orçamentos de saída atualizados",
    "O FastCtx {version} reajustou os orçamentos de saída recomendados por ferramenta. Suas configurações foram atualizadas para os novos padrões. Execute Apply novamente para gravá-los no Codex.",
    "Confirmar",
    "Enter / Esc  Confirmar",
);
const RU: MigrationMessages = migration_messages!(
    "Бюджеты вывода обновлены",
    "FastCtx {version} изменил рекомендуемые бюджеты вывода для отдельных инструментов. Настройки обновлены до новых значений по умолчанию. Повторите Apply, чтобы записать их в Codex.",
    "Подтвердить",
    "Enter / Esc  Подтвердить",
);
const IT: MigrationMessages = migration_messages!(
    "Budget di output aggiornati",
    "FastCtx {version} ha ricalibrato i budget di output consigliati per ogni strumento. Le impostazioni sono state aggiornate ai nuovi valori predefiniti. Esegui di nuovo Apply per scriverli in Codex.",
    "Conferma",
    "Enter / Esc  Conferma",
);
const TR: MigrationMessages = migration_messages!(
    "Çıktı bütçeleri güncellendi",
    "FastCtx {version}, araç başına önerilen çıktı bütçelerini yeniden ayarladı. Ayarlarınız yeni varsayılanlara güncellendi. Bunları Codex'e yazmak için yeniden Apply çalıştırın.",
    "Onayla",
    "Enter / Esc  Onayla",
);
const PL: MigrationMessages = migration_messages!(
    "Zaktualizowano limity wyjścia",
    "FastCtx {version} ponownie wyważył zalecane limity wyjścia dla poszczególnych narzędzi. Ustawienia zaktualizowano do nowych wartości domyślnych. Uruchom ponownie Apply, aby zapisać je w Codex.",
    "Potwierdź",
    "Enter / Esc  Potwierdź",
);
const NL: MigrationMessages = migration_messages!(
    "Uitvoerbudgetten bijgewerkt",
    "FastCtx {version} heeft de aanbevolen uitvoerbudgetten per hulpmiddel opnieuw afgestemd. Je instellingen zijn bijgewerkt naar de nieuwe standaardwaarden. Voer Apply opnieuw uit om ze naar Codex te schrijven.",
    "Bevestigen",
    "Enter / Esc  Bevestigen",
);
const VI: MigrationMessages = migration_messages!(
    "Đã cập nhật ngân sách đầu ra",
    "FastCtx {version} đã điều chỉnh lại ngân sách đầu ra đề xuất cho từng công cụ. Cài đặt của bạn đã được cập nhật sang các giá trị mặc định mới. Hãy Apply lại để ghi chúng vào Codex.",
    "Xác nhận",
    "Enter / Esc  Xác nhận",
);
const ID: MigrationMessages = migration_messages!(
    "Anggaran keluaran diperbarui",
    "FastCtx {version} menyeimbangkan ulang anggaran keluaran yang disarankan untuk tiap alat. Pengaturan Anda telah diperbarui ke nilai bawaan baru. Jalankan Apply lagi untuk menuliskannya ke Codex.",
    "Konfirmasi",
    "Enter / Esc  Konfirmasi",
);
const UK: MigrationMessages = migration_messages!(
    "Бюджети виводу оновлено",
    "FastCtx {version} переналаштував рекомендовані бюджети виводу для окремих інструментів. Налаштування оновлено до нових типових значень. Повторіть Apply, щоб записати їх у Codex.",
    "Підтвердити",
    "Enter / Esc  Підтвердити",
);

pub(crate) const fn messages(language: Language) -> &'static MigrationMessages {
    match language {
        Language::En => &EN,
        Language::ZhCn => &ZH_CN,
        Language::ZhTw => &ZH_TW,
        Language::Ja => &JA,
        Language::Ko => &KO,
        Language::Es => &ES,
        Language::Fr => &FR,
        Language::De => &DE,
        Language::PtBr => &PT_BR,
        Language::Ru => &RU,
        Language::It => &IT,
        Language::Tr => &TR,
        Language::Pl => &PL,
        Language::Nl => &NL,
        Language::Vi => &VI,
        Language::Id => &ID,
        Language::Uk => &UK,
    }
}

#[cfg(test)]
mod tests {
    use super::messages;
    use crate::control::i18n::ALL_LANGUAGES;

    #[test]
    fn every_migration_locale_is_complete_and_keeps_the_version_placeholder() {
        for language in ALL_LANGUAGES {
            let messages = messages(language);
            for value in [
                messages.title,
                messages.body,
                messages.action_confirm,
                messages.footer_confirm,
            ] {
                assert!(!value.trim().is_empty(), "{}", language.code());
            }
            assert_eq!(
                messages.body.matches("{version}").count(),
                1,
                "{}",
                language.code()
            );
            assert!(messages.footer_confirm.contains("Enter"));
            assert!(messages.footer_confirm.contains("Esc"));
        }
    }
}
