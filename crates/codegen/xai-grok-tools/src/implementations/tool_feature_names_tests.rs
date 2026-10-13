use xai_grok_config::ToolFeature;
use xai_tool_runtime::Tool;

use super::{grok_build, opencode};

#[test]
fn tool_features_name_the_tools_they_remove() {
    let cases = [
        (
            ToolFeature::AskUserQuestion,
            vec![grok_build::AskUserQuestionTool.id()],
        ),
        (ToolFeature::ImageEdit, vec![grok_build::ImageEditTool.id()]),
        (ToolFeature::ImageGen, vec![grok_build::ImageGenTool.id()]),
        (ToolFeature::LspTools, vec![grok_build::LspTool.id()]),
        (
            ToolFeature::VideoGen,
            vec![
                grok_build::ImageToVideoTool.id(),
                grok_build::ReferenceToVideoTool.id(),
            ],
        ),
        (ToolFeature::WebFetch, vec![grok_build::WebFetchTool.id()]),
        (
            ToolFeature::WriteFile,
            vec![opencode::OpenCodeWriteTool.id()],
        ),
    ];
    for (feature, tools) in cases {
        let names: Vec<&str> = tools.iter().map(|id| id.as_str()).collect();
        assert_eq!(names, feature.tool_names(), "{feature:?}");
    }
}
