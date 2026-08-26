pub trait WakeSpaceMembershipMaintenancePort: Send + Sync {
    fn wake(&self);
}

impl WakeSpaceMembershipMaintenancePort for crate::space::membership::SpaceMembershipActivity {
    fn wake(&self) {
        let _ = self.request_state_changed();
    }
}
