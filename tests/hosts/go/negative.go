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

	// 宿主错误：目录能力失败以同一稳定分类原样返回；安全存储失败发生在启动内部，
	// 由 Engine 归类为稳定的启动失败（1101），宿主回调确实被调用过。
	denied, _ := newFileHost(filepath.Join(root, "denied"))
	denied.failDirectory = engine.ErrHostPermissionDenied
	_, err = openEngine(denied, manifest)
	result.record("host_directory_error_roundtrip", errors.Is(err, engine.ErrHostPermissionDenied), errString(err))
	failing, _ := newFileHost(filepath.Join(root, "failing"))
	failing.failSet = engine.ErrHostIO
	_, err = openEngine(failing, manifest)
	var startup *engine.EngineError
	result.record("host_secure_storage_failure_is_stable_startup_error",
		errors.As(err, &startup) && startup.Code == 1101 && failing.setCalls.Load() > 0,
		map[string]any{"error": errString(err), "set_calls": failing.setCalls.Load()})

	eng, err := openEngine(host, manifest)
	if !result.record("open_for_negative_checks", err == nil, errString(err)) {
		return
	}
	ctx := context.Background()
	result.record("zero_enum_rejected_before_rust", errors.Is(eng.NotifyConnectivityOpportunity(ctx, 0), engine.ErrInvalidInput), nil)
	result.record("out_of_range_enum_rejected", errors.Is(eng.NotifyConnectivityOpportunity(ctx, 99), engine.ErrInvalidInput), nil)
	result.record("valid_enum_accepted", eng.NotifyConnectivityOpportunity(ctx, engine.OpportunityNetworkChanged) == nil, nil)

	// context 取消只停止等待：Engine 之后仍可用。
	// 先取尽已排队的事件，之后 NextEvent 只能等到 ctx 期限。
	short, cancel := context.WithTimeout(ctx, 600*time.Millisecond)
	for err = nil; err == nil; {
		_, err = eng.NextEvent(short)
	}
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
			for j := 0; j < 100000; j++ {
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
				case isShutdownRace(err):
					// 调用在 Close 之前已进入 Rust，与关闭竞争，得到 Engine 的稳定 InvalidState 错误。
					outcomes["engine_invalid_state_during_shutdown"]++
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

	// 期限极短的 Close：未完成时返回明确错误、仍拒绝新调用，并可用新期限继续收尾直至成功。
	slowHost, _ := newFileHost(filepath.Join(root, "deadline"))
	slow, err := openEngine(slowHost, manifest)
	if result.record("open_for_deadline_retry", err == nil, errString(err)) {
		first := slow.Close(time.Millisecond)
		_, during := slow.LocalDevice(ctx)
		second := slow.Close(20 * time.Second)
		_, after := slow.LocalDevice(ctx)
		result.record("close_deadline_then_retry_succeeds",
			second == nil && errors.Is(during, engine.ErrClosed) && errors.Is(after, engine.ErrClosed) &&
				(first == nil || errors.Is(first, engine.ErrCloseIncomplete)),
			map[string]any{"first_close": errString(first), "second_close": errString(second), "retry_path_exercised": first != nil})
	}

	// 多个 goroutine 同时 Close：全部返回，至多一个真正执行关闭，没有 panic。
	multi, err := openEngine(mustHost(filepath.Join(root, "multi-close")), manifest)
	if result.record("open_for_concurrent_close", err == nil, errString(err)) {
		var group sync.WaitGroup
		results := make([]error, 8)
		panics := 0
		var panicMu sync.Mutex
		for i := range results {
			group.Add(1)
			go func(i int) {
				defer group.Done()
				defer func() {
					if recover() != nil {
						panicMu.Lock()
						panics++
						panicMu.Unlock()
					}
				}()
				results[i] = multi.Close(20 * time.Second)
			}(i)
		}
		group.Wait()
		allNil := true
		for _, closeErr := range results {
			allNil = allNil && closeErr == nil
		}
		result.record("concurrent_close_is_serialized", panics == 0 && allNil, map[string]any{"panics": panics})
	}

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

func isShutdownRace(err error) bool {
	var typed *engine.EngineError
	return errors.As(err, &typed) && typed.Category == engine.CategoryInvalidState
}

func mustHost(root string) *fileHost {
	host, err := newFileHost(root)
	if err != nil {
		panic(err)
	}
	return host
}

// runVerify 只核对原生库来源，不启动 Engine；供没有沙箱的平台（Linux CI）确认库身份解析正确。
func runVerify(result *report, manifest string) {
	_, err := native.Verify(manifest)
	result.record("native_verify_loaded_library", err == nil, errString(err))
}
