use chrono::{Datelike, NaiveDate};

use crate::types::Message;

// DeepSeek validates the language name and the weekday exactly as spelled here (case included).
const LANGUAGES: [(&str, [&str; 7]); 10] = [
    ("English", ["Monday", "Tuesday", "Wednesday", "Thursday", "Friday", "Saturday", "Sunday"]),
    ("español", ["Lunes", "Martes", "Miércoles", "Jueves", "Viernes", "Sábado", "Domingo"]),
    ("português", ["Segunda-feira", "Terça-feira", "Quarta-feira", "Quinta-feira", "Sexta-feira", "Sábado", "Domingo"]),
    ("français", ["Lundi", "Mardi", "Mercredi", "Jeudi", "Vendredi", "Samedi", "Dimanche"]),
    ("italiano", ["Lunedì", "Martedì", "Mercoledì", "Giovedì", "Venerdì", "Sabato", "Domenica"]),
    ("Deutsch", ["Montag", "Dienstag", "Mittwoch", "Donnerstag", "Freitag", "Samstag", "Sonntag"]),
    ("русский", ["Понедельник", "Вторник", "Среда", "Четверг", "Пятница", "Суббота", "Воскресенье"]),
    ("中文", ["星期一", "星期二", "星期三", "星期四", "星期五", "星期六", "星期日"]),
    ("日本語", ["月曜日", "火曜日", "水曜日", "木曜日", "金曜日", "土曜日", "日曜日"]),
    ("한국어", ["월요일", "화요일", "수요일", "목요일", "금요일", "토요일", "일요일"]),
];

const STOPWORDS: [(&str, &[&str]); 5] = [
    ("español", &["que", "el", "los", "las", "una", "para", "por", "con", "como", "pero", "esto", "hay", "está", "qué", "dame", "haz", "quiero", "mira", "donde"]),
    ("português", &["não", "você", "uma", "para", "com", "isso", "está", "obrigado", "fazer", "também", "mas", "então"]),
    ("français", &["le", "les", "une", "est", "pour", "avec", "pas", "vous", "je", "mais", "c'est", "dans"]),
    ("italiano", &["il", "che", "una", "per", "con", "non", "sono", "questo", "anche", "perché", "gli"]),
    ("Deutsch", &["der", "die", "das", "und", "ist", "nicht", "ich", "mit", "ein", "eine", "auch", "bitte"]),
];

pub fn language_of(text: &str) -> &'static str {
    if text.chars().any(|c| ('\u{AC00}'..='\u{D7AF}').contains(&c)) {
        return "한국어";
    }
    if text.chars().any(crate::language::is_japanese_kana) {
        return "日本語";
    }
    if crate::language::contains_cjk(text) {
        return "中文";
    }
    if text.chars().any(|c| ('\u{0400}'..='\u{04FF}').contains(&c)) {
        return "русский";
    }
    let lower = text.to_lowercase();
    let words: Vec<&str> = lower.split(|c: char| !c.is_alphanumeric() && c != '\'').filter(|w| !w.is_empty()).collect();
    STOPWORDS
        .iter()
        .map(|(lang, list)| (*lang, words.iter().filter(|w| list.contains(w)).count()))
        .filter(|(_, hits)| *hits >= 2)
        .max_by_key(|(_, hits)| *hits)
        .map_or("English", |(lang, _)| lang)
}

pub fn reminder_text(date: NaiveDate, language: &str) -> String {
    let (name, days) = LANGUAGES.iter().find(|(name, _)| *name == language).unwrap_or(&LANGUAGES[0]);
    let weekday = days[date.weekday().num_days_from_monday() as usize];
    format!("{},{weekday},,App,{name}", date.format("%Y-%m-%d"))
}

/// Puts today's date before the newest real user message, the way DeepSeek Chat tells its model.
/// Ignores synthetic tool-image user messages so loading an image mid-turn never shifts the
/// reminder (which would invalidate DeepSeek's KV prefix cache and misdetect the language).
pub fn insert_latest_reminder(messages: &mut Vec<Message>, today: NaiveDate) {
    let Some(idx) = messages
        .iter()
        .rposition(|m| {
            m.role == "user"
                && !crate::tool_images::is_tool_image_message(m)
                && !m
                    .text_content()
                    .is_some_and(|t| t.starts_with("TASK_DONE|") || t.starts_with("SUBAGENT_DONE|"))
        })
    else {
        return;
    };
    let language = messages[idx]
        .display_text()
        .as_deref()
        .map_or("English", language_of);
    messages.insert(
        idx,
        Message {
            role: "latest_reminder".to_string(),
            content: Some(crate::types::MessageContent::Text(reminder_text(today, language))),
            ..Default::default()
        },
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    fn day(d: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(2026, 11, d).unwrap()
    }

    #[test]
    fn the_reminder_spells_the_weekday_in_the_users_language() {
        assert_eq!(reminder_text(day(18), "español"), "2026-11-18,Miércoles,,App,español");
        assert_eq!(reminder_text(day(16), "English"), "2026-11-16,Monday,,App,English");
        assert_eq!(reminder_text(day(22), "中文"), "2026-11-22,星期日,,App,中文");
        assert_eq!(reminder_text(day(18), "klingon"), "2026-11-18,Wednesday,,App,English");
    }

    #[test]
    fn the_language_comes_from_the_words_of_the_message() {
        assert_eq!(language_of("bro mira, aveces el modelo se detiene sin que yo lo cancele"), "español");
        assert_eq!(language_of("why does the stream stop?"), "English");
        assert_eq!(language_of("请帮我看看"), "中文");
        assert_eq!(language_of("これは何ですか"), "日本語");
        assert_eq!(language_of("ok"), "English");
    }

    #[test]
    fn the_reminder_goes_right_before_the_newest_user_message() {
        let mut messages = vec![
            Message::system("sys"),
            Message::user("hola, qué hay para hoy"),
            Message::assistant("Listo".to_string(), None),
            Message::tool_response("c1".to_string(), "ok".to_string()),
        ];
        insert_latest_reminder(&mut messages, day(18));
        assert_eq!(messages[1].role, "latest_reminder");
        assert_eq!(messages[1].text_content(), Some("2026-11-18,Miércoles,,App,español"));
        assert_eq!(messages[2].role, "user");

        let mut none = vec![Message::system("sys")];
        insert_latest_reminder(&mut none, day(18));
        assert_eq!(none.len(), 1);
    }

    #[test]
    fn synthetic_tool_image_messages_do_not_move_reminder_or_reset_language() {
        use crate::types::ContentPart;
        let mut messages = vec![
            Message::system("sys"),
            Message::user_parts(vec![
                ContentPart::text("mira esta foto por favor [Image #1]"),
                ContentPart::image_url("data:image/png;base64,AAAA"),
            ]),
            Message::assistant("Voy a comparar".to_string(), None),
            Message::user_parts(vec![
                ContentPart::text("[Image(s) loaded by the read_image tool: /tmp/a.png]"),
                ContentPart::image_url("data:image/png;base64,BBBB"),
            ]),
        ];
        insert_latest_reminder(&mut messages, day(18));
        assert_eq!(messages[1].role, "latest_reminder");
        assert_eq!(messages[1].text_content(), Some("2026-11-18,Miércoles,,App,español"));
    }
}
