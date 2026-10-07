package main

import (
	"context"
	"errors"
	"os"
	"path/filepath"
	"time"

	"github.com/UniClipboard/Engine/bindings/go/engine"
)

const stepTimeout = 30 * time.Second

func openEngine(host *fileHost, manifest string) (*engine.Engine, error) {
	return engine.Open(engine.Config{
		AppVersion:     appVersion,
		ProfileID:      profileID,
		NativeManifest: manifest,
	}, host)
}

// drainStates 读取事件直到出现期望状态或超时，返回观察到的状态序列。
func drainStates(eng *engine.Engine, want engine.State) ([]engine.State, bool) {
	ctx, cancel := context.WithTimeout(context.Background(), stepTimeout)
	defer cancel()
	var seen []engine.State
	for {
		event, err := eng.NextEvent(ctx)
		if err != nil {
			return seen, false
		}
		if changed, ok := event.(engine.StateChanged); ok {
			seen = append(seen, changed.State)
			if changed.State == want {
				return seen, true
			}
		}
	}
}

func runLifecycle(result *report, root, manifest string) {
	host, err := newFileHost(root)
	if err != nil || !result.record("host_ready", err == nil, nil) {
		return
	}
	if err := installObservability(host, appVersion); !result.record("observability_local_only", err == nil, errString(err)) {
		return
	}
	eng, err := openEngine(host, manifest)
	if !result.record("open_after_native_verify", err == nil, errString(err)) {
		return
	}
	ctx, cancel := context.WithTimeout(context.Background(), stepTimeout)
	defer cancel()

	device, err := eng.LocalDevice(ctx)
	if !result.record("query_local_device", err == nil && device.DeviceID != "", errString(err)) {
		return
	}
	space, err := eng.SpaceState(ctx)
	result.record("query_space_state_fresh_profile", err == nil && !space.HasCompleted, map[string]any{"has_completed": space.HasCompleted})

	leaveErr := eng.LeaveSpace(ctx)
	var typed *engine.EngineError
	if errors.As(leaveErr, &typed) {
		result.record("typed_engine_error", true, map[string]any{"code": typed.Code, "category": typed.Category, "retryable": typed.Retryable})
	} else {
		result.record("typed_engine_error", false, errString(leaveErr))
	}

	counters := host.counters()
	result.record("host_callbacks_succeeded", counters["private_data_directory"] > 0 && counters["secure_storage_get"] > 0 && counters["secure_storage_set"] > 0, counters)

	suspendErr := eng.Suspend(ctx)
	_, suspended := drainStates(eng, engine.StateSuspended)
	result.record("suspend_emits_state_changed", suspendErr == nil && suspended, errString(suspendErr))
	resumeErr := eng.Resume(ctx)
	_, running := drainStates(eng, engine.StateRunning)
	result.record("resume_emits_state_changed", resumeErr == nil && running, errString(resumeErr))

	closeErr := eng.Close(15 * time.Second)
	result.record("shutdown_with_deadline_and_join", closeErr == nil, errString(closeErr))
	_, afterErr := eng.LocalDevice(ctx)
	result.record("call_after_close_is_rejected", errors.Is(afterErr, engine.ErrClosed), errString(afterErr))
	result.record("close_is_idempotent", eng.Close(time.Second) == nil, nil)

	state, _ := marshalJSON(map[string]string{"device_id": device.DeviceID})
	if err := os.WriteFile(filepath.Join(root, "device.json"), state, 0o600); err != nil {
		result.record("persist_device_identity", false, errString(err))
	}
	hits, scanErr := scanForLeaks(root, []string{profileID}, host.secrets)
	result.record("no_secret_in_logs_or_profile", scanErr == nil && len(hits) == 0, map[string]any{"hits": hits})
}

func runRestart(result *report, root, manifest string) {
	host, err := newFileHost(root)
	if err != nil || !result.record("host_ready", err == nil, nil) {
		return
	}
	previous, err := os.ReadFile(filepath.Join(root, "device.json"))
	if !result.record("previous_identity_present", err == nil, errString(err)) {
		return
	}
	eng, err := openEngine(host, manifest)
	if !result.record("reopen_same_profile", err == nil, errString(err)) {
		return
	}
	ctx, cancel := context.WithTimeout(context.Background(), stepTimeout)
	defer cancel()
	device, err := eng.LocalDevice(ctx)
	expected, _ := marshalJSON(map[string]string{"device_id": device.DeviceID})
	result.record("same_device_identity_after_restart", err == nil && string(expected) == string(previous), errString(err))
	result.record("close_after_restart", eng.Close(15*time.Second) == nil, nil)
	hits, scanErr := scanForLeaks(root, []string{profileID}, host.secrets)
	result.record("no_secret_in_logs_or_profile", scanErr == nil && len(hits) == 0, map[string]any{"hits": hits})
}

func errString(err error) any {
	if err == nil {
		return nil
	}
	return err.Error()
}
