//! Localized strings for search parallelism and destructive settings reset.

use super::i18n::Language;

/// Complete CPU-limit editor and reset string set for one language.
#[derive(Debug)]
pub(crate) struct ConfigMessages {
    pub(crate) search_group_title: &'static str,
    pub(crate) cpu_limit_label: &'static str,
    pub(crate) cpu_automatic: &'static str,
    pub(crate) cpu_limit_note: &'static str,
    pub(crate) cpu_edit_title: &'static str,
    pub(crate) cpu_edit_prompt: &'static str,
    pub(crate) cpu_error_empty: &'static str,
    pub(crate) cpu_error_not_integer: &'static str,
    pub(crate) cpu_error_range: &'static str,
    pub(crate) reset_group_title: &'static str,
    pub(crate) reset_all_label: &'static str,
    pub(crate) reset_all_note: &'static str,
    pub(crate) reset_confirm: &'static str,
    pub(crate) reset_success: &'static str,
    pub(crate) footer_edit: &'static str,
    pub(crate) footer_accept: &'static str,
}

impl ConfigMessages {
    #[cfg(test)]
    fn values(&self) -> [&'static str; 16] {
        [
            self.search_group_title,
            self.cpu_limit_label,
            self.cpu_automatic,
            self.cpu_limit_note,
            self.cpu_edit_title,
            self.cpu_edit_prompt,
            self.cpu_error_empty,
            self.cpu_error_not_integer,
            self.cpu_error_range,
            self.reset_group_title,
            self.reset_all_label,
            self.reset_all_note,
            self.reset_confirm,
            self.reset_success,
            self.footer_edit,
            self.footer_accept,
        ]
    }
}

/// Returns the complete configuration-extension table for a supported language.
pub(crate) const fn messages(language: Language) -> &'static ConfigMessages {
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

macro_rules! config_messages {
    (
        $search_group_title:expr, $cpu_limit_label:expr, $cpu_automatic:expr,
        $cpu_limit_note:expr, $cpu_edit_title:expr, $cpu_edit_prompt:expr,
        $cpu_error_empty:expr, $cpu_error_not_integer:expr, $cpu_error_range:expr,
        $reset_group_title:expr, $reset_all_label:expr, $reset_all_note:expr,
        $reset_confirm:expr, $reset_success:expr, $footer_edit:expr, $footer_accept:expr
    ) => {
        ConfigMessages {
            search_group_title: $search_group_title,
            cpu_limit_label: $cpu_limit_label,
            cpu_automatic: $cpu_automatic,
            cpu_limit_note: $cpu_limit_note,
            cpu_edit_title: $cpu_edit_title,
            cpu_edit_prompt: $cpu_edit_prompt,
            cpu_error_empty: $cpu_error_empty,
            cpu_error_not_integer: $cpu_error_not_integer,
            cpu_error_range: $cpu_error_range,
            reset_group_title: $reset_group_title,
            reset_all_label: $reset_all_label,
            reset_all_note: $reset_all_note,
            reset_confirm: $reset_confirm,
            reset_success: $reset_success,
            footer_edit: $footer_edit,
            footer_accept: $footer_accept,
        }
    };
}

const EN: ConfigMessages = config_messages!(
    "Search",
    "CPU limit",
    "Automatic",
    "Used by grep/glob; applies to newly started server processes. Apply is not required.",
    "Edit CPU limit",
    "Enter auto or 1..={maximum}:",
    "Value cannot be empty.",
    "Enter auto or a whole number.",
    "Enter auto or a number from 1 to {maximum}.",
    "Reset",
    "Reset all settings",
    "Keeps the Apply receipt and running jobs; may evict the oldest finished jobs above the default history quota.",
    "Reset all settings?",
    "All settings reset.",
    "Edit",
    "Accept"
);

const ZH_CN: ConfigMessages = config_messages!(
    "搜索",
    "CPU 限制",
    "自动",
    "用于 grep/glob；仅对新启动的 server 进程生效。无需 Apply。",
    "编辑 CPU 限制",
    "输入 auto 或 1..={maximum}：",
    "值不能为空。",
    "请输入 auto 或整数。",
    "请输入 auto 或 1 到 {maximum} 之间的数字。",
    "重置",
    "重置所有设置",
    "保留 Apply receipt 和正在运行的 jobs；恢复默认历史配额时，可能回收超额的最旧 finished jobs。",
    "要重置所有设置吗？",
    "已重置所有设置。",
    "编辑",
    "确认"
);

const ZH_TW: ConfigMessages = config_messages!(
    "搜尋",
    "CPU 限制",
    "自動",
    "用於 grep/glob；僅對新啟動的 server 程序生效。不需要 Apply。",
    "編輯 CPU 限制",
    "輸入 auto 或 1..={maximum}：",
    "值不可為空。",
    "請輸入 auto 或整數。",
    "請輸入 auto 或 1 到 {maximum} 之間的數字。",
    "重設",
    "重設所有設定",
    "保留 Apply receipt 和執行中的 jobs；恢復預設歷史配額時，可能回收超額的最舊 finished jobs。",
    "要重設所有設定嗎？",
    "已重設所有設定。",
    "編輯",
    "確認"
);

const JA: ConfigMessages = config_messages!(
    "検索",
    "CPU 制限",
    "自動",
    "grep/glob で使用。新しく起動した server プロセスにのみ適用されます。Apply は不要です。",
    "CPU 制限を編集",
    "auto または 1..={maximum} を入力:",
    "値は空にできません。",
    "auto または整数を入力してください。",
    "auto または 1～{maximum} の数値を入力してください。",
    "リセット",
    "すべての設定をリセット",
    "Apply receipt と実行中の jobs は保持されます。既定の履歴クォータへ戻す際、超過分の古い finished jobs が削除される場合があります。",
    "すべての設定をリセットしますか？",
    "すべての設定をリセットしました。",
    "編集",
    "確定"
);

const KO: ConfigMessages = config_messages!(
    "검색",
    "CPU 제한",
    "자동",
    "grep/glob에 사용되며 새로 시작한 server 프로세스에만 적용됩니다. Apply는 필요하지 않습니다.",
    "CPU 제한 편집",
    "auto 또는 1..={maximum} 입력:",
    "값을 비워 둘 수 없습니다.",
    "auto 또는 정수를 입력하세요.",
    "auto 또는 1~{maximum} 사이의 숫자를 입력하세요.",
    "재설정",
    "모든 설정 재설정",
    "Apply receipt와 실행 중인 jobs는 유지됩니다. 기본 기록 할당량으로 되돌릴 때 초과된 오래된 finished jobs가 삭제될 수 있습니다.",
    "모든 설정을 재설정할까요?",
    "모든 설정을 재설정했습니다.",
    "편집",
    "확인"
);

const ES: ConfigMessages = config_messages!(
    "Búsqueda",
    "Límite de CPU",
    "Automático",
    "Se usa para grep/glob; solo se aplica a procesos server iniciados después. No hace falta Apply.",
    "Editar límite de CPU",
    "Introduce auto o 1..={maximum}:",
    "El valor no puede estar vacío.",
    "Introduce auto o un número entero.",
    "Introduce auto o un número del 1 al {maximum}.",
    "Restablecer",
    "Restablecer todos los ajustes",
    "Conserva el Apply receipt y los jobs en ejecución; al volver a la cuota de historial predeterminada puede expulsar los finished jobs más antiguos que la superen.",
    "¿Restablecer todos los ajustes?",
    "Todos los ajustes se restablecieron.",
    "Editar",
    "Aceptar"
);

const FR: ConfigMessages = config_messages!(
    "Recherche",
    "Limite du processeur",
    "Automatique",
    "Utilisé par grep/glob ; s’applique aux processus server démarrés ensuite. Apply n’est pas nécessaire.",
    "Modifier la limite du processeur",
    "Saisissez auto ou 1..={maximum} :",
    "La valeur ne peut pas être vide.",
    "Saisissez auto ou un nombre entier.",
    "Saisissez auto ou un nombre entre 1 et {maximum}.",
    "Réinitialiser",
    "Réinitialiser tous les réglages",
    "Conserve l’Apply receipt et les jobs en cours ; le retour au quota d’historique par défaut peut supprimer les finished jobs les plus anciens au-delà de ce quota.",
    "Réinitialiser tous les réglages ?",
    "Tous les réglages ont été réinitialisés.",
    "Modifier",
    "Valider"
);

const DE: ConfigMessages = config_messages!(
    "Suche",
    "CPU-Limit",
    "Automatisch",
    "Für grep/glob verwendet; gilt nur für neu gestartete server-Prozesse. Apply ist nicht erforderlich.",
    "CPU-Limit bearbeiten",
    "auto oder 1..={maximum} eingeben:",
    "Der Wert darf nicht leer sein.",
    "Geben Sie auto oder eine ganze Zahl ein.",
    "Geben Sie auto oder eine Zahl zwischen 1 und {maximum} ein.",
    "Zurücksetzen",
    "Alle Einstellungen zurücksetzen",
    "Behält den Apply receipt und laufende jobs; beim Zurücksetzen auf das Standard-Verlaufskontingent können die ältesten finished jobs darüber entfernt werden.",
    "Alle Einstellungen zurücksetzen?",
    "Alle Einstellungen wurden zurückgesetzt.",
    "Bearbeiten",
    "Übernehmen"
);

const PT_BR: ConfigMessages = config_messages!(
    "Pesquisa",
    "Limite de CPU",
    "Automático",
    "Usado por grep/glob; aplica apenas a processos server iniciados depois. Não é necessário usar Apply.",
    "Editar limite de CPU",
    "Digite auto ou 1..={maximum}:",
    "O valor não pode ficar vazio.",
    "Digite auto ou um número inteiro.",
    "Digite auto ou um número de 1 a {maximum}.",
    "Redefinir",
    "Redefinir todas as configurações",
    "Mantém o Apply receipt e os jobs em execução; ao restaurar a cota de histórico padrão, pode remover os finished jobs mais antigos que a excederem.",
    "Redefinir todas as configurações?",
    "Todas as configurações foram redefinidas.",
    "Editar",
    "Aceitar"
);

const RU: ConfigMessages = config_messages!(
    "Поиск",
    "Лимит ЦП",
    "Автоматически",
    "Используется grep/glob; действует только для вновь запущенных процессов server. Нажимать Apply не нужно.",
    "Изменить лимит ЦП",
    "Введите auto или 1..={maximum}:",
    "Значение не может быть пустым.",
    "Введите auto или целое число.",
    "Введите auto или число от 1 до {maximum}.",
    "Сброс",
    "Сбросить все настройки",
    "Сохраняет Apply receipt и запущенные jobs; при возврате к стандартной квоте истории могут быть удалены самые старые finished jobs сверх неё.",
    "Сбросить все настройки?",
    "Все настройки сброшены.",
    "Изменить",
    "Принять"
);

const IT: ConfigMessages = config_messages!(
    "Ricerca",
    "Limite CPU",
    "Automatico",
    "Usato da grep/glob; vale solo per i processi server avviati successivamente. Apply non è necessario.",
    "Modifica limite CPU",
    "Inserisci auto o 1..={maximum}:",
    "Il valore non può essere vuoto.",
    "Inserisci auto o un numero intero.",
    "Inserisci auto o un numero da 1 a {maximum}.",
    "Ripristina",
    "Ripristina tutte le impostazioni",
    "Mantiene l’Apply receipt e i jobs in esecuzione; ripristinando la quota di cronologia predefinita può rimuovere i finished jobs più vecchi che la superano.",
    "Ripristinare tutte le impostazioni?",
    "Tutte le impostazioni sono state ripristinate.",
    "Modifica",
    "Accetta"
);

const TR: ConfigMessages = config_messages!(
    "Arama",
    "CPU sınırı",
    "Otomatik",
    "grep/glob tarafından kullanılır; yalnızca yeni başlatılan server işlemlerine uygulanır. Apply gerekmez.",
    "CPU sınırını düzenle",
    "auto veya 1..={maximum} girin:",
    "Değer boş bırakılamaz.",
    "auto veya tam sayı girin.",
    "auto veya 1 ile {maximum} arasında bir sayı girin.",
    "Sıfırla",
    "Tüm ayarları sıfırla",
    "Apply receipt ve çalışan jobs korunur; varsayılan geçmiş kotasına dönerken kotayı aşan en eski finished jobs kaldırılabilir.",
    "Tüm ayarlar sıfırlansın mı?",
    "Tüm ayarlar sıfırlandı.",
    "Düzenle",
    "Kabul et"
);

const PL: ConfigMessages = config_messages!(
    "Wyszukiwanie",
    "Limit CPU",
    "Automatycznie",
    "Używany przez grep/glob; dotyczy tylko nowo uruchomionych procesów server. Apply nie jest wymagane.",
    "Edytuj limit CPU",
    "Wpisz auto lub 1..={maximum}:",
    "Wartość nie może być pusta.",
    "Wpisz auto lub liczbę całkowitą.",
    "Wpisz auto lub liczbę od 1 do {maximum}.",
    "Resetuj",
    "Zresetuj wszystkie ustawienia",
    "Zachowuje Apply receipt i uruchomione jobs; przywrócenie domyślnego limitu historii może usunąć najstarsze finished jobs ponad ten limit.",
    "Zresetować wszystkie ustawienia?",
    "Zresetowano wszystkie ustawienia.",
    "Edytuj",
    "Akceptuj"
);

const NL: ConfigMessages = config_messages!(
    "Zoeken",
    "CPU-limiet",
    "Automatisch",
    "Gebruikt door grep/glob; geldt alleen voor nieuw gestarte serverprocessen. Apply is niet nodig.",
    "CPU-limiet bewerken",
    "Voer auto of 1..={maximum} in:",
    "De waarde mag niet leeg zijn.",
    "Voer auto of een geheel getal in.",
    "Voer auto of een getal van 1 tot {maximum} in.",
    "Resetten",
    "Alle instellingen resetten",
    "Behoudt de Apply receipt en actieve jobs; bij herstel van het standaard geschiedenisquotum kunnen de oudste finished jobs daarboven worden verwijderd.",
    "Alle instellingen resetten?",
    "Alle instellingen zijn gereset.",
    "Bewerken",
    "Accepteren"
);

const VI: ConfigMessages = config_messages!(
    "Tìm kiếm",
    "Giới hạn CPU",
    "Tự động",
    "Dùng cho grep/glob; chỉ áp dụng cho các tiến trình server khởi động mới. Không cần Apply.",
    "Chỉnh giới hạn CPU",
    "Nhập auto hoặc 1..={maximum}:",
    "Giá trị không được để trống.",
    "Nhập auto hoặc số nguyên.",
    "Nhập auto hoặc số từ 1 đến {maximum}.",
    "Đặt lại",
    "Đặt lại mọi cài đặt",
    "Giữ Apply receipt và các jobs đang chạy; khi khôi phục hạn mức lịch sử mặc định, các finished jobs cũ nhất vượt hạn mức có thể bị xóa.",
    "Đặt lại mọi cài đặt?",
    "Đã đặt lại mọi cài đặt.",
    "Chỉnh sửa",
    "Chấp nhận"
);

const ID: ConfigMessages = config_messages!(
    "Pencarian",
    "Batas CPU",
    "Otomatis",
    "Digunakan oleh grep/glob; hanya berlaku untuk proses server yang baru dijalankan. Apply tidak diperlukan.",
    "Edit batas CPU",
    "Masukkan auto atau 1..={maximum}:",
    "Nilai tidak boleh kosong.",
    "Masukkan auto atau bilangan bulat.",
    "Masukkan auto atau angka dari 1 hingga {maximum}.",
    "Atur ulang",
    "Atur ulang semua pengaturan",
    "Mempertahankan Apply receipt dan jobs yang berjalan; saat memulihkan kuota riwayat default, finished jobs terlama di atas kuota dapat dihapus.",
    "Atur ulang semua pengaturan?",
    "Semua pengaturan diatur ulang.",
    "Edit",
    "Terima"
);

const UK: ConfigMessages = config_messages!(
    "Пошук",
    "Ліміт ЦП",
    "Автоматично",
    "Використовується grep/glob; діє лише для щойно запущених процесів server. Apply не потрібен.",
    "Редагувати ліміт ЦП",
    "Введіть auto або 1..={maximum}:",
    "Значення не може бути порожнім.",
    "Введіть auto або ціле число.",
    "Введіть auto або число від 1 до {maximum}.",
    "Скидання",
    "Скинути всі налаштування",
    "Зберігає Apply receipt і запущені jobs; під час повернення до типової квоти історії найстаріші finished jobs понад неї можуть бути видалені.",
    "Скинути всі налаштування?",
    "Усі налаштування скинуто.",
    "Редагувати",
    "Прийняти"
);

#[cfg(test)]
mod tests {
    use super::messages;
    use crate::control::i18n::ALL_LANGUAGES;

    #[test]
    fn every_language_has_complete_cpu_reset_strings_and_exact_range_placeholders() {
        for language in ALL_LANGUAGES {
            let messages = messages(language);
            assert!(
                messages
                    .values()
                    .iter()
                    .all(|value| !value.trim().is_empty()),
                "{} has an empty configuration translation",
                language.code()
            );
            assert_eq!(messages.cpu_edit_prompt.matches("{maximum}").count(), 1);
            assert_eq!(messages.cpu_error_range.matches("{maximum}").count(), 1);
            assert!(messages.cpu_limit_note.contains("grep/glob"));
            assert!(messages.cpu_limit_note.contains("Apply"));
            assert!(messages.reset_all_note.contains("Apply receipt"));
            assert!(messages.reset_all_note.contains("jobs"));
            assert!(
                messages.reset_all_note.contains("finished"),
                "{} reset note must disclose finished-job reclamation",
                language.code()
            );
        }
    }
}
