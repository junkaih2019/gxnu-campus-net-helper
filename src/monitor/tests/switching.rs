use super::*;
use crate::metrics_store::{outages_in_range, TimeRange};

struct TestNetwork {
    result: Mutex<DiagnosticResult>,
    route: Mutex<RoutePath>,
    scan_gate: Mutex<Option<(mpsc::Sender<()>, Receiver<()>)>>,
}

impl TestNetwork {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            result: Mutex::new(online()),
            route: Mutex::new(fixture_route()),
            scan_gate: Mutex::new(None),
        })
    }

    fn block_next_scan(&self) -> (Receiver<()>, mpsc::Sender<()>) {
        let (entered, observing) = mpsc::channel();
        let (release, gate) = mpsc::channel();
        *self.scan_gate.lock().unwrap() = Some((entered, gate));
        (observing, release)
    }
}

impl NetworkCheck for Arc<TestNetwork> {
    fn route(&self) -> RoutePath {
        *self.route.lock().unwrap()
    }
    fn check_on_route(&self, _path: RoutePath) -> DiagnosticResult {
        let result = self.result.lock().unwrap().clone();
        if let Some((entered, release)) = self.scan_gate.lock().unwrap().take() {
            entered.send(()).unwrap();
            release.recv_timeout(Duration::from_secs(3)).unwrap();
        }
        result
    }

    fn portal_latency(&self) -> Option<u64> {
        None
    }
}

fn online() -> DiagnosticResult {
    DiagnosticResult::new(true, DisconnectReason::Online, "online")
}

fn fixture() -> (Fixture, Arc<TestNetwork>) {
    let net = TestNetwork::new();
    let f = Fixture::with_check(Arc::clone(&net), "", Arc::new(|_| Some(fake_credential())));
    f.monitor.control().start().unwrap();
    f.until(|s| s.state == MonitorState::Online);
    (f, net)
}

fn outage_count(f: &Fixture) -> usize {
    let points = MetricsStore::history(f._dir.path());
    outages_in_range(&points, chrono::Local::now().timestamp(), TimeRange::H1)
        .starts
        .len()
}

#[test]
fn transient_failures_during_manual_and_scheduled_checks_never_alert_login_or_count() {
    for reason in [
        DisconnectReason::UplinkUnreachable,
        DisconnectReason::NoGateway,
        DisconnectReason::InternetTimeout,
    ] {
        let (f, net) = fixture();
        let ctrl = f.monitor.control();
        *net.result.lock().unwrap() = DiagnosticResult::new(false, reason, "switching")
            .authenticatable(reason == DisconnectReason::InternetTimeout);
        ctrl.network_changed();
        ctrl.force_check().unwrap();
        let early = f.until(|s| s.metrics.check_seq == 2);
        assert_eq!(early.state, MonitorState::Checking);
        assert!(early.metrics.last_point.is_none());
        f.advance(2);
        let retry = f.until(|s| s.metrics.check_seq == 3);
        assert_eq!(retry.state, MonitorState::Checking);
        *net.result.lock().unwrap() = online();
        f.advance(3);
        f.until(|s| s.state == MonitorState::Online && s.metrics.check_seq == 4);
        ctrl.pause().unwrap(); // Flush the final online RTT before inspecting history.
        assert!(f.notices.lock().unwrap().is_empty());
        f.assert_no_submission();
        assert_eq!(outage_count(&f), 0);
        let points = MetricsStore::history(f._dir.path());
        assert_eq!(points.len(), 2);
        assert!(points.iter().all(|p| p.online && p.event == "check"));
    }
}

#[test]
fn persistent_link_loss_and_online_portal_outage_each_alert_once_without_login() {
    for reason in [
        DisconnectReason::LinkDown,
        DisconnectReason::UplinkUnreachable,
    ] {
        let (f, net) = fixture();
        let ctrl = f.monitor.control();
        *net.result.lock().unwrap() = DiagnosticResult::new(false, reason, "unreachable");
        ctrl.network_changed();
        f.advance(1);
        f.until(|s| s.metrics.check_seq == 2 && s.state == MonitorState::Checking);
        assert!(f.notices.lock().unwrap().is_empty());
        f.advance(4);
        let offline = f.until(|s| s.state == MonitorState::Offline);
        assert_eq!(offline.reason, Some(reason));
        ctrl.force_check().unwrap();
        f.until(|s| {
            s.state == MonitorState::Offline && s.metrics.check_seq > offline.metrics.check_seq
        });
        assert_eq!(outage_count(&f), 1);
        assert_eq!(f.notices.lock().unwrap().len(), 1);
        f.assert_no_submission();
    }
}

#[test]
fn offline_portal_on_settled_route_logs_in_and_verifies_recovery() {
    let (f, net) = fixture();
    let ctrl = f.monitor.control();
    let mut offline = DiagnosticResult::new(false, DisconnectReason::AuthExpired, "expired")
        .authenticatable(true);
    offline.push_evidence("campus_status", "offline");
    *net.result.lock().unwrap() = offline;
    ctrl.network_changed();
    f.advance(1);
    f.until(|s| s.metrics.check_seq == 2 && s.state == MonitorState::Checking);
    f.assert_no_submission();
    assert!(f.notices.lock().unwrap().is_empty());
    f.advance(4);
    f.until(|s| s.state == MonitorState::Authenticating);
    *net.result.lock().unwrap() = online();
    reply_once(&f.portal, r#"dr1004({"result":1})"#);
    f.until(|s| s.state == MonitorState::Online);
    assert_eq!(outage_count(&f), 1);
    assert_eq!(f.notices.lock().unwrap().len(), 2); // offline, then verified recovery
    f.assert_no_submission();
}

fn tun_route() -> RoutePath {
    RoutePath::Ipv4 {
        interface: 2,
        source: [198, 18, 0, 1].into(),
        next_hop: [198, 18, 0, 2].into(),
    }
}

fn wait_switch(f: &Fixture) -> MonitorSnapshot {
    f.until(|s| s.state == MonitorState::Checking && s.message == "网络切换中，正在复查连接")
}

#[test]
fn route_change_during_scan_discards_old_result_even_without_a_windows_event() {
    let (f, net) = fixture();
    let ctrl = f.monitor.control();
    *net.result.lock().unwrap() =
        DiagnosticResult::new(false, DisconnectReason::InternetTimeout, "stale")
            .authenticatable(true);
    let (entered, release) = net.block_next_scan();
    ctrl.force_check().unwrap();
    entered.recv_timeout(Duration::from_secs(3)).unwrap();
    *net.route.lock().unwrap() = tun_route();
    release.send(()).unwrap();
    let switching = wait_switch(&f);
    assert_eq!(switching.metrics.check_seq, 1);
    f.advance(1);
    f.until(|s| s.state == MonitorState::Checking && s.metrics.check_seq == 2);
    *net.result.lock().unwrap() = online();
    f.advance(1);
    f.until(|s| s.state == MonitorState::Online && s.metrics.check_seq == 3);
    assert!(f.notices.lock().unwrap().is_empty());
    assert_eq!(outage_count(&f), 0);
    f.assert_no_submission();
}

#[test]
fn missed_interface_source_gateway_and_route_loss_between_cycles_open_a_new_window() {
    let RoutePath::Ipv4 {
        interface,
        source,
        next_hop,
    } = fixture_route()
    else {
        unreachable!()
    };
    for path in [
        RoutePath::Ipv4 {
            interface: 42,
            source,
            next_hop,
        },
        RoutePath::Ipv4 {
            interface,
            source: [192, 0, 2, 9].into(),
            next_hop,
        },
        RoutePath::Ipv4 {
            interface,
            source,
            next_hop: [192, 0, 2, 99].into(),
        },
        RoutePath::Unavailable,
    ] {
        let (f, net) = fixture();
        *net.route.lock().unwrap() = path;
        *net.result.lock().unwrap() =
            DiagnosticResult::new(false, DisconnectReason::NoGateway, "changing");
        f.monitor.control().force_check().unwrap();
        assert_eq!(wait_switch(&f).metrics.check_seq, 1);
        f.advance(1);
        let provisional = f.until(|s| s.metrics.check_seq == 2);
        assert_eq!(provisional.state, MonitorState::Checking);
        assert_eq!(outage_count(&f), 0);
        assert!(f.notices.lock().unwrap().is_empty());
        f.assert_no_submission();
    }
}

#[test]
fn a_silent_route_change_while_reading_credentials_cancels_submission() {
    let (entered, reading) = mpsc::channel();
    let (release, gate) = mpsc::channel();
    let gate = Mutex::new(gate);
    let net = TestNetwork::new();
    let f = Fixture::with_check(
        Arc::clone(&net),
        "",
        Arc::new(move |_| {
            entered.send(()).unwrap();
            gate.lock()
                .unwrap()
                .recv_timeout(Duration::from_secs(3))
                .unwrap();
            Some(fake_credential())
        }),
    );
    let ctrl = f.monitor.control();
    ctrl.start().unwrap();
    f.until(|s| s.state == MonitorState::Online);
    *net.result.lock().unwrap() =
        DiagnosticResult::new(false, DisconnectReason::AuthExpired, "expired")
            .authenticatable(true);
    ctrl.force_check().unwrap();
    reading.recv_timeout(Duration::from_secs(3)).unwrap();
    *net.route.lock().unwrap() = tun_route();
    *net.result.lock().unwrap() = online();
    release.send(()).unwrap();
    wait_switch(&f);
    f.advance(1);
    f.until(|s| s.state == MonitorState::Online);
    f.assert_no_submission();
}

#[test]
fn a_slow_provisional_scan_cannot_authorize_itself_when_the_grace_period_expires() {
    let (f, net) = fixture();
    let ctrl = f.monitor.control();
    *net.result.lock().unwrap() =
        DiagnosticResult::new(false, DisconnectReason::InternetTimeout, "unknown portal")
            .authenticatable(true);
    let (entered, release) = net.block_next_scan();
    ctrl.network_changed();
    f.advance(1);
    entered.recv_timeout(Duration::from_secs(3)).unwrap();
    f.advance(10);
    *net.result.lock().unwrap() = online();
    release.send(()).unwrap();
    f.until(|s| s.state == MonitorState::Online && s.metrics.check_seq >= 3);
    assert!(f.notices.lock().unwrap().is_empty());
    assert_eq!(outage_count(&f), 0);
    f.assert_no_submission();
}

#[test]
fn verification_cannot_attribute_recovery_to_a_login_on_a_previous_route() {
    let (f, net) = fixture();
    *net.result.lock().unwrap() =
        DiagnosticResult::new(false, DisconnectReason::AuthExpired, "expired")
            .authenticatable(true);
    f.monitor.control().force_check().unwrap();
    f.until(|s| s.state == MonitorState::Authenticating);
    *net.result.lock().unwrap() = online();
    let (entered, release) = net.block_next_scan();
    reply_once(&f.portal, r#"dr1004({"result":1})"#);
    entered.recv_timeout(Duration::from_secs(3)).unwrap();
    *net.route.lock().unwrap() = tun_route();
    release.send(()).unwrap();
    wait_switch(&f);
    f.advance(1);
    let recovered = f.until(|s| s.state == MonitorState::Online);
    assert_eq!(recovered.message, "online");
    assert!(MetricsStore::history(f._dir.path())
        .iter()
        .all(|p| p.event != "login_ok"));
}
