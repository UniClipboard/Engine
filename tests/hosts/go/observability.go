package main

import (
	"bytes"
	"encoding/base64"
	"encoding/hex"
	"os"
	"path/filepath"

	ffi "github.com/UniClipboard/Engine/bindings/go/uc_engine_uniffi"
)

// ffiHost 把 fileHost 暴露为生成层的完整宿主接口，仅用于安装进程级观测：
// 本地文件观测写入隔离根目录，远端导出关闭。门面本身不开放观测安装，验收宿主
// 直接使用生成层以取得 Rust 侧的原始日志。
type ffiHost struct{ *fileHost }

func (h ffiHost) SecureStorageGet(key string) (*[]byte, error) {
	value, err := h.fileHost.SecureStorageGet(key)
	if err != nil || value == nil {
		return nil, err
	}
	return &value, nil
}

func (ffiHost) FileMetadata(string) (ffi.BindingFileMetadata, error) {
	return ffi.BindingFileMetadata{}, ffi.NewHostBindingErrorUnavailable()
}

func (ffiHost) FileReadChunk(string, uint64, uint32) ([]byte, error) {
	return nil, ffi.NewHostBindingErrorUnavailable()
}

func (ffiHost) FileWriteChunk(string, uint64, []byte) error {
	return ffi.NewHostBindingErrorUnavailable()
}

func (ffiHost) FileFinishWrite(string) error { return ffi.NewHostBindingErrorUnavailable() }

func (ffiHost) ClipboardRead() (ffi.BindingClipboardSnapshot, error) {
	return ffi.BindingClipboardSnapshot{}, ffi.NewHostBindingErrorUnavailable()
}

func (ffiHost) ClipboardWrite(ffi.BindingClipboardSnapshot) error {
	return ffi.NewHostBindingErrorUnavailable()
}

func installObservability(host *fileHost, version string) error {
	_, err := ffi.InstallProcessObservability(ffi.BindingObservabilityConfig{
		ServiceVersion:           version,
		Environment:              ffi.BindingDeploymentEnvironmentTest,
		AppChannel:               "go-binding-acceptance",
		RemoteDiagnosticsEnabled: false,
		Collector:                nil,
	}, ffiHost{host})
	return err
}

// scanForLeaks 在隔离根目录的全部文件（Rust 写入的观测日志等，不含安全存储本身）中查找哨兵与密钥字节
// 的原文、hex、base64 形式，返回命中的文件相对路径与种类；不输出命中的内容。
func scanForLeaks(root string, sentinels []string, secrets [][]byte) ([]string, error) {
	var needles []struct {
		kind string
		data []byte
	}
	for _, sentinel := range sentinels {
		needles = append(needles, struct {
			kind string
			data []byte
		}{"sentinel", []byte(sentinel)})
	}
	for _, secret := range secrets {
		if len(secret) < 8 {
			continue
		}
		for kind, data := range map[string][]byte{
			"secret-raw":    secret,
			"secret-hex":    []byte(hex.EncodeToString(secret)),
			"secret-base64": []byte(base64.StdEncoding.EncodeToString(secret)),
		} {
			needles = append(needles, struct {
				kind string
				data []byte
			}{kind, data})
		}
	}
	var hits []string
	err := filepath.Walk(root, func(path string, info os.FileInfo, err error) error {
		if err != nil || info.IsDir() {
			return err
		}
		rel, _ := filepath.Rel(root, path)
		if filepath.Dir(rel) == "secure" {
			return nil
		}
		content, err := os.ReadFile(path)
		if err != nil {
			return err
		}
		for _, needle := range needles {
			if bytes.Contains(content, needle.data) {
				hits = append(hits, rel+":"+needle.kind)
			}
		}
		return nil
	})
	return hits, err
}
