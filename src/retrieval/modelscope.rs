use std::{
    error::Error,
    fmt, fs,
    path::{Path, PathBuf},
};

use fastembed::{Pooling, TokenizerFiles, UserDefinedEmbeddingModel};

use crate::runtime_tools::{RuntimeToolError, install_verified_file};

const MODEL_REPOSITORY: &str = "Xenova/bge-small-zh-v1.5";
const MODEL_REVISION: &str = "08d7186b7de51be7c12444137221ad96825593d6";

const MODEL_ASSETS: [ModelAsset; 5] = [
    ModelAsset {
        remote_path: "onnx/model.onnx",
        local_name: "model.onnx",
        bytes: 94_851_877,
        sha256: "69a0b846f4f116b5e6aabf9546ea6754d02264f3211a13a1bd69b31b8040749a",
    },
    ModelAsset {
        remote_path: "tokenizer.json",
        local_name: "tokenizer.json",
        bytes: 439_125,
        sha256: "48cea5d44424912a6fd1ea647bf4fe50b55ab8b1e5879c3275f80e339e8fae26",
    },
    ModelAsset {
        remote_path: "config.json",
        local_name: "config.json",
        bytes: 716,
        sha256: "d4193ead3a810fd694fa8a31d7fc72fbaebc0668b603e398734bf2f6538ff42f",
    },
    ModelAsset {
        remote_path: "special_tokens_map.json",
        local_name: "special_tokens_map.json",
        bytes: 125,
        sha256: "b6d346be366a7d1d48332dbc9fdf3bf8960b5d879522b7799ddba59e76237ee3",
    },
    ModelAsset {
        remote_path: "tokenizer_config.json",
        local_name: "tokenizer_config.json",
        bytes: 367,
        sha256: "e6f3b96db926a37d4039995fbf5ad17de158dfb8f6343d607e4dbaad18d75f5a",
    },
];

#[derive(Debug, Clone, Copy)]
struct ModelAsset {
    remote_path: &'static str,
    local_name: &'static str,
    bytes: u64,
    sha256: &'static str,
}

impl ModelAsset {
    fn url(self) -> String {
        format!(
            "https://modelscope.cn/models/{MODEL_REPOSITORY}/resolve/{MODEL_REVISION}/{}",
            self.remote_path
        )
    }
}

pub(super) fn load_embedding_model() -> Result<UserDefinedEmbeddingModel, ModelScopeModelError> {
    let cache_root = dirs::cache_dir().ok_or(ModelScopeModelError::NoCacheDirectory)?;
    load_embedding_model_from(&cache_root.join("beyond-slides").join("models"))
}

fn load_embedding_model_from(
    models_root: &Path,
) -> Result<UserDefinedEmbeddingModel, ModelScopeModelError> {
    let model_directory =
        models_root.join(format!("bge-small-zh-v1.5-modelscope-{MODEL_REVISION}"));
    for asset in MODEL_ASSETS {
        let destination = model_directory.join(asset.local_name);
        install_verified_file(&destination, &asset.url(), asset.bytes, asset.sha256).map_err(
            |source| ModelScopeModelError::Install {
                file: asset.remote_path,
                source,
            },
        )?;
    }

    let read = |file_name: &'static str| {
        let path = model_directory.join(file_name);
        fs::read(&path).map_err(|source| ModelScopeModelError::Read { path, source })
    };
    let onnx_file = read("model.onnx")?;
    let tokenizer_files = TokenizerFiles {
        tokenizer_file: read("tokenizer.json")?,
        config_file: read("config.json")?,
        special_tokens_map_file: read("special_tokens_map.json")?,
        tokenizer_config_file: read("tokenizer_config.json")?,
    };

    Ok(UserDefinedEmbeddingModel::new(onnx_file, tokenizer_files).with_pooling(Pooling::Cls))
}

#[derive(Debug)]
pub(super) enum ModelScopeModelError {
    NoCacheDirectory,
    Install {
        file: &'static str,
        source: RuntimeToolError,
    },
    Read {
        path: PathBuf,
        source: std::io::Error,
    },
}

impl fmt::Display for ModelScopeModelError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NoCacheDirectory => formatter.write_str(
                "could not determine the user cache directory for the BGE embedding model",
            ),
            Self::Install { file, source } => write!(
                formatter,
                "could not install BGE embedding asset {file} from ModelScope: {source}"
            ),
            Self::Read { path, source } => write!(
                formatter,
                "could not read cached BGE embedding asset {}: {source}",
                path.display()
            ),
        }
    }
}

impl Error for ModelScopeModelError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Install { source, .. } => Some(source),
            Self::Read { source, .. } => Some(source),
            Self::NoCacheDirectory => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn model_assets_are_pinned_to_modelscope_revision() {
        assert_eq!(MODEL_ASSETS.len(), 5);
        for asset in MODEL_ASSETS {
            assert_eq!(asset.sha256.len(), 64);
            assert!(asset.bytes > 0);
            assert_eq!(
                asset.url(),
                format!(
                    "https://modelscope.cn/models/Xenova/bge-small-zh-v1.5/resolve/{MODEL_REVISION}/{}",
                    asset.remote_path
                )
            );
        }
    }
}
