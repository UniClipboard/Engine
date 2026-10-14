package main

import (
	"crypto/sha256"
	"encoding/hex"
	"errors"
	"os"
	"path/filepath"
	"sync"
	"sync/atomic"

	"github.com/UniClipboard/Engine/bindings/go/engine"
)

// fileHost 是验收用的隔离宿主：目录与安全存储都在一个临时根目录内，
// 安全存储以文件持久化，使第二个进程可以在同一 profile 上重启。
// 它不接触真实 profile、钥匙串、剪贴板或网络。
type fileHost struct {
	root string

	privateCalls atomic.Int64
	cacheCalls   atomic.Int64
	tempCalls    atomic.Int64
	getCalls     atomic.Int64
	setCalls     atomic.Int64
	deleteCalls  atomic.Int64

	mu         sync.Mutex
	storedKeys map[string]bool
	// failSet 非空时，安全存储写入返回该错误，用于验证宿主错误往返。
	failSet error
	// failDirectory 非空时，三个目录能力返回该错误。
	failDirectory error
	// secrets 记录 Rust 写入的全部密文字节，供日志扫描使用。
	secrets [][]byte
}

func newFileHost(root string) (*fileHost, error) {
	for _, dir := range []string{"private", "cache", "temp", "secure"} {
		if err := os.MkdirAll(filepath.Join(root, dir), 0o700); err != nil {
			return nil, err
		}
	}
	return &fileHost{root: root, storedKeys: map[string]bool{}}, nil
}

func (h *fileHost) PrivateDataDirectory() (string, error) {
	if h.failDirectory != nil {
		return "", h.failDirectory
	}
	h.privateCalls.Add(1)
	return filepath.Join(h.root, "private"), nil
}

func (h *fileHost) CacheDirectory() (string, error) {
	if h.failDirectory != nil {
		return "", h.failDirectory
	}
	h.cacheCalls.Add(1)
	return filepath.Join(h.root, "cache"), nil
}

func (h *fileHost) TemporaryDirectory() (string, error) {
	if h.failDirectory != nil {
		return "", h.failDirectory
	}
	h.tempCalls.Add(1)
	return filepath.Join(h.root, "temp"), nil
}

func (h *fileHost) securePath(key string) string {
	sum := sha256.Sum256([]byte(key))
	return filepath.Join(h.root, "secure", hex.EncodeToString(sum[:]))
}

func (h *fileHost) SecureStorageGet(key string) ([]byte, error) {
	h.getCalls.Add(1)
	value, err := os.ReadFile(h.securePath(key))
	if errors.Is(err, os.ErrNotExist) {
		return nil, nil
	}
	if err != nil {
		return nil, engine.ErrHostIO
	}
	return value, nil
}

func (h *fileHost) SecureStorageSet(key string, value []byte) error {
	h.setCalls.Add(1)
	h.mu.Lock()
	defer h.mu.Unlock()
	if h.failSet != nil {
		return h.failSet
	}
	if err := os.WriteFile(h.securePath(key), value, 0o600); err != nil {
		return engine.ErrHostIO
	}
	h.secrets = append(h.secrets, append([]byte(nil), value...))
	return nil
}

func (h *fileHost) SecureStorageDelete(key string) error {
	h.deleteCalls.Add(1)
	if err := os.Remove(h.securePath(key)); err != nil && !errors.Is(err, os.ErrNotExist) {
		return engine.ErrHostIO
	}
	return nil
}

func (h *fileHost) counters() map[string]int64 {
	return map[string]int64{
		"private_data_directory": h.privateCalls.Load(),
		"cache_directory":        h.cacheCalls.Load(),
		"temporary_directory":    h.tempCalls.Load(),
		"secure_storage_get":     h.getCalls.Load(),
		"secure_storage_set":     h.setCalls.Load(),
		"secure_storage_delete":  h.deleteCalls.Load(),
	}
}
