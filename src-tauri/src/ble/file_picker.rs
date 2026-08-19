use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "lowercase")]
pub enum PickerResult {
    Pending,
    Selected { path: String },
    Cancelled,
    Error { message: String },
    Missing,
}

pub fn parse_picker_result(raw: &str) -> Result<PickerResult, String> {
    serde_json::from_str(raw).map_err(|error| format!("解析文件选择状态失败: {error}"))
}

#[cfg(test)]
mod tests {
    use super::{parse_picker_result, PickerResult};

    #[test]
    fn parses_selected_file_with_unicode_name() {
        let result = parse_picker_result(r#"{"status":"selected","path":"芙宁娜表盘静态.bin"}"#)
            .expect("应解析成功");

        assert_eq!(
            result,
            PickerResult::Selected {
                path: "芙宁娜表盘静态.bin".into(),
            }
        );
    }

    #[test]
    fn parses_all_terminal_and_waiting_states() {
        assert_eq!(
            parse_picker_result(r#"{"status":"pending"}"#).unwrap(),
            PickerResult::Pending
        );
        assert_eq!(
            parse_picker_result(r#"{"status":"cancelled"}"#).unwrap(),
            PickerResult::Cancelled
        );
        assert_eq!(
            parse_picker_result(r#"{"status":"missing"}"#).unwrap(),
            PickerResult::Missing
        );
        assert_eq!(
            parse_picker_result(r#"{"status":"error","message":"启动失败"}"#).unwrap(),
            PickerResult::Error {
                message: "启动失败".into(),
            }
        );
    }

    #[test]
    fn rejects_unknown_picker_status() {
        let error = parse_picker_result(r#"{"status":"done"}"#).unwrap_err();
        assert!(error.contains("解析文件选择状态失败"));
    }
}
