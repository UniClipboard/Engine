package main

import (
	"context"
	"errors"
	"os"
	"path/filepath"
	"sync"
	"time"

	"github.com/UniClipboard/Engine/bindings/go/engine"
	"github.com/UniClipboard/Engine/bindings/go/native"
)

func runNegative(result *report, root, manifest string) {
	// 输入校验：零值枚举与空配置在进入 Rust 之前被拒绝。
	_, err := engine.Open(engine.Config{}, nil)
	result.record("open_rejects_empty_config", errors.Is(err, engine.ErrInvalidInput), errString(err))

	// 清单缺失、哈希被篡改：Open 必须拒绝，且不启动 Engine。
	missing := filepath.Join(root, "absent-manifest.json")
	host, err := newFileHost(root)
	if err != nil {
		result.record("host_ready", false, errString(err))
		return
	}
	_, err = openEngine(host, missing)
	result.record("open_rejects_missing_manifest", errors.Is(err, native.ErrMismatch), errString(err))
	tampered, err := tamperedManifest(manifest, root)
	if err == nil {
		_, err = openEngine(host, tampered)
	}
	result.record("open_rejects_digest_mismatch", errors.Is(err, native.ErrMismatch), errString(err))
	result.record("no_engine_started_by_rejected_open", host.counters()["private_data_directory"] == 0, host.counters())

	// 宿主错误往返：安全存储写入被拒绝，Open 返回同一稳定分类。
	failing, _ := newFileHost(filepath.Join(root, "failing"))
	failing.failSet = engine.ErrHostPermissionDenied
	_, err = openEngine(failing, manifest)
	result.record("host_error_roundtrip", errors.Is(err, engine.ErrHostPermissionDenied), errString(err))

	eng, err := openEngine(host, manifest)
	if !result.record("open_for_negative_checks", err == nil, errString(err)) {
		return
	}
	ctx := context.Background()
	result.record("zero_enum_rejected_before_rust", errors.Is(eng.NotifyConnectivityOpportunity(ctx, 0), engine.ErrInvalidInput), nil)
	result.record("out_of_range_enum_rejected", errors.Is(eng.NotifyConnectivityOpportunity(ctx, 99), engine.ErrInvalidInput), nil)
	result.record("valid_enum_accepted", eng.NotifyConnectivityOpportunity(ctx, engine.OpportunityNetworkChanged) == nil, nil)

	// context 取消只停止等待：Engine 之后仍可用。
	short, cancel := context.WithTimeout(ctx, 300*time.Millisecond)
	_, err = eng.NextEvent(short)
	cancel()
	result.record("context_deadline_stops_waiting", errors.Is(err, context.DeadlineExceeded), errString(err))
	_, err = eng.LocalDevice(ctx)
	result.record("engine_usable_after_context_deadline", err == nil, errString(err))

	// 并发调用与 Close：全部调用得到结果或 ErrClosed，没有 panic，Close 在期限内完成。
	var wg sync.WaitGroup
	var mu sync.Mutex
	outcomes := map[string]int{}
	for i := 0; i < 32; i++ {
		wg.Add(1)
		go func(i int) {
			defer wg.Done()
			defer func() {
				if recover() != nil {
					mu.Lock()
					outcomes["panic"]++
					mu.Unlock()
				}
			}()
			for j := 0; j < 50; j++ {
				var err error
				if i%4 == 0 {
					short, cancel := context.WithTimeout(ctx, 100*time.Millisecond)
					_, err = eng.NextEvent(short)
					cancel()
					if errors.Is(err, context.DeadlineExceeded) {
						err = nil
					}
				} else {
					_, err = eng.LocalDevice(ctx)
				}
				mu.Lock()
				switch {
				case err == nil:
					outcomes["ok"]++
				case errors.Is(err, engine.ErrClosed):
					outcomes["closed"]++
				default:
					outcomes["other:"+err.Error()]++
				}
				mu.Unlock()
				if errors.Is(err, engine.ErrClosed) {
					return
				}
			}
		}(i)
	}
	time.Sleep(150 * time.Millisecond)
	started := time.Now()
	closeErr := eng.Close(20 * time.Second)
	closeTook := time.Since(started)
	wg.Wait()
	mu.Lock()
	clean := outcomes["panic"] == 0
	for key := range outcomes {
		if len(key) > 6 && key[:6] == "other:" {
			clean = false
		}
	}
	snapshot := map[string]any{"outcomes": outcomes, "close_ms": closeTook.Milliseconds()}
	mu.Unlock()
	result.record("concurrent_calls_with_close", closeErr == nil && clean, snapshot)
	_, err = eng.NextEvent(ctx)
	result.record("next_event_after_close_is_rejected", errors.Is(err, engine.ErrClosed), errString(err))

	hits, scanErr := scanForLeaks(root, []string{profileID}, host.secrets)
	result.record("no_secret_in_logs_or_profile", scanErr == nil && len(hits) == 0, map[string]any{"hits": hits})
}

// tamperedManifest 复制清单并改动库摘要，验证 Verify 逐项比对而不是只检查存在。
func tamperedManifest(path, root string) (string, error) {
	data, err := os.ReadFile(path)
	if err != nil {
		return "", err
	}
	var parsed map[string]any
	if err := unmarshalJSON(data, &parsed); err != nil {
		return "", err
	}
	library := parsed["library"].(map[string]any)
	digest := library["sha256"].(string)
	library["sha256"] = "0" + digest[1:]
	if digest[0] == '0' {
		library["sha256"] = "1" + digest[1:]
	}
	out, err := marshalJSON(parsed)
	if err != nil {
		return "", err
	}
	target := filepath.Join(root, "tampered-manifest.json")
	return target, os.WriteFile(target, out, 0o600)
}
