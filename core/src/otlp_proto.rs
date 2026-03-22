/// Zero-dependency OTLP protobuf encoder.
///
/// Implements the minimal protobuf wire format needed to encode
/// `ExportTraceServiceRequest`, `ExportMetricsServiceRequest`, and
/// `ExportLogsServiceRequest` from the existing JSON OTLP types.

use crate::telemetry::*;

// ── Hex decoder ──

fn hex_decode(s: &str) -> Vec<u8> {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len() / 2);
    let mut i = 0;
    while i + 1 < bytes.len() {
        let hi = hex_nibble(bytes[i]);
        let lo = hex_nibble(bytes[i + 1]);
        out.push((hi << 4) | lo);
        i += 2;
    }
    out
}

fn hex_nibble(b: u8) -> u8 {
    match b {
        b'0'..=b'9' => b - b'0',
        b'a'..=b'f' => b - b'a' + 10,
        b'A'..=b'F' => b - b'A' + 10,
        _ => 0,
    }
}

// ── Protobuf wire format writer ──

#[allow(dead_code)]
pub struct ProtobufWriter {
    buf: Vec<u8>,
}

impl ProtobufWriter {
    pub fn new() -> Self {
        Self { buf: Vec::new() }
    }

    /// Append a raw varint (no tag).
    fn write_raw_varint(&mut self, mut val: u64) {
        loop {
            let byte = (val & 0x7F) as u8;
            val >>= 7;
            if val == 0 {
                self.buf.push(byte);
                break;
            }
            self.buf.push(byte | 0x80);
        }
    }

    fn write_tag(&mut self, field: u32, wire_type: u8) {
        self.write_raw_varint(((field as u64) << 3) | wire_type as u64);
    }

    /// Field: varint (wire type 0).
    pub fn write_varint(&mut self, field: u32, val: u64) {
        if val == 0 {
            return;
        }
        self.write_tag(field, 0);
        self.write_raw_varint(val);
    }

    /// Field: string (wire type 2).
    pub fn write_string(&mut self, field: u32, val: &str) {
        if val.is_empty() {
            return;
        }
        self.write_tag(field, 2);
        self.write_raw_varint(val.len() as u64);
        self.buf.extend_from_slice(val.as_bytes());
    }

    /// Field: bytes (wire type 2).
    pub fn write_bytes(&mut self, field: u32, val: &[u8]) {
        if val.is_empty() {
            return;
        }
        self.write_tag(field, 2);
        self.write_raw_varint(val.len() as u64);
        self.buf.extend_from_slice(val);
    }

    /// Field: fixed64 (wire type 1).
    pub fn write_fixed64(&mut self, field: u32, val: u64) {
        if val == 0 {
            return;
        }
        self.write_tag(field, 1);
        self.buf.extend_from_slice(&val.to_le_bytes());
    }

    /// Field: double (wire type 1).
    pub fn write_double(&mut self, field: u32, val: f64) {
        if val == 0.0 {
            return;
        }
        self.write_tag(field, 1);
        self.buf.extend_from_slice(&val.to_le_bytes());
    }

    /// Field: sfixed64 (wire type 1).
    pub fn write_sfixed64(&mut self, field: u32, val: i64) {
        if val == 0 {
            return;
        }
        self.write_tag(field, 1);
        self.buf.extend_from_slice(&val.to_le_bytes());
    }

    /// Field: bool as varint.
    pub fn write_bool(&mut self, field: u32, val: bool) {
        if !val {
            return;
        }
        self.write_tag(field, 0);
        self.buf.push(1);
    }

    /// Field: embedded message (wire type 2).
    pub fn write_message(&mut self, field: u32, inner: &ProtobufWriter) {
        if inner.buf.is_empty() {
            return;
        }
        self.write_tag(field, 2);
        self.write_raw_varint(inner.buf.len() as u64);
        self.buf.extend_from_slice(&inner.buf);
    }

    pub fn into_bytes(self) -> Vec<u8> {
        self.buf
    }
}

// ── Common OTLP type encoders ──

fn encode_any_value(v: &OtlpAnyValue) -> ProtobufWriter {
    let mut w = ProtobufWriter::new();
    // AnyValue oneof: string_value=1, bool_value=2, int_value=3, double_value=4
    if let Some(ref s) = v.string_value {
        w.write_string(1, s);
    } else if let Some(b) = v.bool_value {
        // bool_value field 2 — must write even if false since it's a oneof
        w.write_tag(2, 0);
        w.buf.push(if b { 1 } else { 0 });
    } else if let Some(i) = v.int_value {
        // int_value field 3 — must write even if 0 since it's a oneof
        w.write_tag(3, 0);
        w.write_raw_varint(i as u64);
    } else if let Some(d) = v.double_value {
        // double_value field 4 — must write even if 0.0 since it's a oneof
        w.write_tag(4, 1);
        w.buf.extend_from_slice(&d.to_le_bytes());
    }
    w
}

fn encode_key_value(kv: &OtlpKeyValue) -> ProtobufWriter {
    let mut w = ProtobufWriter::new();
    // KeyValue { string key = 1; AnyValue value = 2; }
    w.write_string(1, &kv.key);
    let val = encode_any_value(&kv.value);
    w.write_message(2, &val);
    w
}

fn encode_resource(res: &OtlpResource) -> ProtobufWriter {
    let mut w = ProtobufWriter::new();
    // Resource { repeated KeyValue attributes = 1; }
    for attr in &res.attributes {
        let kv = encode_key_value(attr);
        w.write_message(1, &kv);
    }
    w
}

fn encode_scope(scope: &OtlpScope) -> ProtobufWriter {
    let mut w = ProtobufWriter::new();
    // InstrumentationScope { string name = 1; string version = 2; }
    w.write_string(1, &scope.name);
    w.write_string(2, &scope.version);
    w
}

/// Parse a nanosecond timestamp string to u64.
fn parse_nanos(s: &str) -> u64 {
    s.parse::<u64>().unwrap_or(0)
}

// ── Traces ──

fn encode_span_event(ev: &OtlpSpanEvent) -> ProtobufWriter {
    let mut w = ProtobufWriter::new();
    // Event { fixed64 time_unix_nano = 1; string name = 2; repeated KeyValue attributes = 3; }
    w.write_fixed64(1, parse_nanos(&ev.time_unix_nano));
    w.write_string(2, &ev.name);
    for attr in &ev.attributes {
        let kv = encode_key_value(attr);
        w.write_message(3, &kv);
    }
    w
}

fn encode_span_status(st: &OtlpSpanStatus) -> ProtobufWriter {
    let mut w = ProtobufWriter::new();
    // Status { string message = 2; StatusCode code = 3; }
    if let Some(ref msg) = st.message {
        w.write_string(2, msg);
    }
    w.write_varint(3, st.code as u64);
    w
}

fn encode_span(span: &OtlpSpan) -> ProtobufWriter {
    let mut w = ProtobufWriter::new();
    // bytes trace_id = 1
    let trace_bytes = hex_decode(&span.trace_id);
    w.write_bytes(1, &trace_bytes);
    // bytes span_id = 2
    let span_bytes = hex_decode(&span.span_id);
    w.write_bytes(2, &span_bytes);
    // string trace_state = 3 (unused, skip)
    // bytes parent_span_id = 4
    if let Some(ref pid) = span.parent_span_id {
        let parent_bytes = hex_decode(pid);
        w.write_bytes(4, &parent_bytes);
    }
    // string name = 5
    w.write_string(5, &span.name);
    // SpanKind kind = 6
    w.write_varint(6, span.kind as u64);
    // fixed64 start_time_unix_nano = 7
    w.write_fixed64(7, parse_nanos(&span.start_time_unix_nano));
    // fixed64 end_time_unix_nano = 8
    w.write_fixed64(8, parse_nanos(&span.end_time_unix_nano));
    // repeated KeyValue attributes = 9
    for attr in &span.attributes {
        let kv = encode_key_value(attr);
        w.write_message(9, &kv);
    }
    // repeated Event events = 11
    for ev in &span.events {
        let e = encode_span_event(ev);
        w.write_message(11, &e);
    }
    // Status status = 15
    let status = encode_span_status(&span.status);
    w.write_message(15, &status);
    w
}

fn encode_scope_spans(ss: &OtlpScopeSpans) -> ProtobufWriter {
    let mut w = ProtobufWriter::new();
    let scope = encode_scope(&ss.scope);
    w.write_message(1, &scope);
    for span in &ss.spans {
        let s = encode_span(span);
        w.write_message(2, &s);
    }
    w
}

fn encode_resource_spans(rs: &OtlpResourceSpans) -> ProtobufWriter {
    let mut w = ProtobufWriter::new();
    let resource = encode_resource(&rs.resource);
    w.write_message(1, &resource);
    for ss in &rs.scope_spans {
        let s = encode_scope_spans(ss);
        w.write_message(2, &s);
    }
    w
}

pub fn encode_traces(export: &OtlpTraceExport) -> Vec<u8> {
    let mut w = ProtobufWriter::new();
    for rs in &export.resource_spans {
        let r = encode_resource_spans(rs);
        w.write_message(1, &r);
    }
    w.into_bytes()
}

// ── Metrics ──

fn encode_number_data_point(dp: &OtlpNumberDataPoint) -> ProtobufWriter {
    let mut w = ProtobufWriter::new();
    // fixed64 start_time_unix_nano = 2
    w.write_fixed64(2, parse_nanos(&dp.start_time_unix_nano));
    // fixed64 time_unix_nano = 3
    w.write_fixed64(3, parse_nanos(&dp.time_unix_nano));
    // oneof value: double as_double = 4; sfixed64 as_int = 6
    if let Some(d) = dp.as_double {
        // Write even for 0.0 since it's a oneof
        w.write_tag(4, 1);
        w.buf.extend_from_slice(&d.to_le_bytes());
    } else if let Some(i) = dp.as_int {
        // Write even for 0 since it's a oneof
        w.write_tag(6, 1);
        w.buf.extend_from_slice(&i.to_le_bytes());
    }
    // repeated KeyValue attributes = 7
    for attr in &dp.attributes {
        let kv = encode_key_value(attr);
        w.write_message(7, &kv);
    }
    w
}

fn encode_sum(sum: &OtlpSum) -> ProtobufWriter {
    let mut w = ProtobufWriter::new();
    // repeated NumberDataPoint data_points = 1
    for dp in &sum.data_points {
        let d = encode_number_data_point(dp);
        w.write_message(1, &d);
    }
    // int32 aggregation_temporality = 2
    w.write_varint(2, sum.aggregation_temporality as u64);
    // bool is_monotonic = 3
    w.write_bool(3, sum.is_monotonic);
    w
}

fn encode_histogram_data_point(dp: &OtlpHistogramDataPoint) -> ProtobufWriter {
    let mut w = ProtobufWriter::new();
    // fixed64 start_time_unix_nano = 2
    w.write_fixed64(2, parse_nanos(&dp.start_time_unix_nano));
    // fixed64 time_unix_nano = 3
    w.write_fixed64(3, parse_nanos(&dp.time_unix_nano));
    // uint64 count = 4
    w.write_varint(4, dp.count);
    // optional double sum = 5 — always present in our types
    w.write_tag(5, 1);
    w.buf.extend_from_slice(&dp.sum.to_le_bytes());
    // min/max: double min = 12; double max = 13
    w.write_tag(12, 1);
    w.buf.extend_from_slice(&dp.min.to_le_bytes());
    w.write_tag(13, 1);
    w.buf.extend_from_slice(&dp.max.to_le_bytes());
    // repeated KeyValue attributes = 9
    for attr in &dp.attributes {
        let kv = encode_key_value(attr);
        w.write_message(9, &kv);
    }
    w
}

fn encode_histogram(hist: &OtlpHistogram) -> ProtobufWriter {
    let mut w = ProtobufWriter::new();
    // repeated HistogramDataPoint data_points = 1
    for dp in &hist.data_points {
        let d = encode_histogram_data_point(dp);
        w.write_message(1, &d);
    }
    // int32 aggregation_temporality = 2
    w.write_varint(2, hist.aggregation_temporality as u64);
    w
}

fn encode_metric(m: &OtlpMetric) -> ProtobufWriter {
    let mut w = ProtobufWriter::new();
    // string name = 1
    w.write_string(1, &m.name);
    // string description = 2
    w.write_string(2, &m.description);
    // string unit = 3
    w.write_string(3, &m.unit);
    // oneof data: Sum sum = 7; Histogram histogram = 9
    if let Some(ref sum) = m.sum {
        let s = encode_sum(sum);
        w.write_message(7, &s);
    }
    if let Some(ref hist) = m.histogram {
        let h = encode_histogram(hist);
        w.write_message(9, &h);
    }
    w
}

fn encode_scope_metrics(sm: &OtlpScopeMetrics) -> ProtobufWriter {
    let mut w = ProtobufWriter::new();
    let scope = encode_scope(&sm.scope);
    w.write_message(1, &scope);
    for m in &sm.metrics {
        let metric = encode_metric(m);
        w.write_message(2, &metric);
    }
    w
}

fn encode_resource_metrics(rm: &OtlpResourceMetrics) -> ProtobufWriter {
    let mut w = ProtobufWriter::new();
    let resource = encode_resource(&rm.resource);
    w.write_message(1, &resource);
    for sm in &rm.scope_metrics {
        let s = encode_scope_metrics(sm);
        w.write_message(2, &s);
    }
    w
}

pub fn encode_metrics(export: &OtlpMetricExport) -> Vec<u8> {
    let mut w = ProtobufWriter::new();
    for rm in &export.resource_metrics {
        let r = encode_resource_metrics(rm);
        w.write_message(1, &r);
    }
    w.into_bytes()
}

// ── Logs ──

fn encode_log_record(log: &OtlpLogRecord) -> ProtobufWriter {
    let mut w = ProtobufWriter::new();
    // fixed64 time_unix_nano = 1
    w.write_fixed64(1, parse_nanos(&log.time_unix_nano));
    // SeverityNumber severity_number = 2 (enum)
    w.write_varint(2, log.severity_number as u64);
    // string severity_text = 3
    w.write_string(3, &log.severity_text);
    // AnyValue body = 5
    let body = encode_any_value(&log.body);
    w.write_message(5, &body);
    // repeated KeyValue attributes = 6
    for attr in &log.attributes {
        let kv = encode_key_value(attr);
        w.write_message(6, &kv);
    }
    // bytes trace_id = 9
    if let Some(ref tid) = log.trace_id {
        let bytes = hex_decode(tid);
        w.write_bytes(9, &bytes);
    }
    // bytes span_id = 10
    if let Some(ref sid) = log.span_id {
        let bytes = hex_decode(sid);
        w.write_bytes(10, &bytes);
    }
    // fixed64 observed_time_unix_nano = 11
    // Use same value as time_unix_nano (our types don't distinguish them)
    w.write_fixed64(11, parse_nanos(&log.time_unix_nano));
    w
}

fn encode_scope_logs(sl: &OtlpScopeLogs) -> ProtobufWriter {
    let mut w = ProtobufWriter::new();
    let scope = encode_scope(&sl.scope);
    w.write_message(1, &scope);
    for log in &sl.log_records {
        let l = encode_log_record(log);
        w.write_message(2, &l);
    }
    w
}

fn encode_resource_logs(rl: &OtlpResourceLogs) -> ProtobufWriter {
    let mut w = ProtobufWriter::new();
    let resource = encode_resource(&rl.resource);
    w.write_message(1, &resource);
    for sl in &rl.scope_logs {
        let s = encode_scope_logs(sl);
        w.write_message(2, &s);
    }
    w
}

pub fn encode_logs(export: &OtlpLogExport) -> Vec<u8> {
    let mut w = ProtobufWriter::new();
    for rl in &export.resource_logs {
        let r = encode_resource_logs(rl);
        w.write_message(1, &r);
    }
    w.into_bytes()
}

// ── Tests ──

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_hex_decode() {
        assert_eq!(hex_decode(""), Vec::<u8>::new());
        assert_eq!(hex_decode("00"), vec![0u8]);
        assert_eq!(hex_decode("ff"), vec![255u8]);
        assert_eq!(hex_decode("0123456789abcdef"), vec![0x01, 0x23, 0x45, 0x67, 0x89, 0xab, 0xcd, 0xef]);
        assert_eq!(hex_decode("ABCDEF"), vec![0xab, 0xcd, 0xef]);
    }

    #[test]
    fn test_varint_encoding() {
        let mut w = ProtobufWriter::new();
        w.write_raw_varint(1);
        assert_eq!(w.buf, vec![1]);

        let mut w = ProtobufWriter::new();
        w.write_raw_varint(300);
        assert_eq!(w.buf, vec![0xAC, 0x02]);

        let mut w = ProtobufWriter::new();
        w.write_raw_varint(0);
        assert_eq!(w.buf, vec![0]);
    }

    #[test]
    fn test_write_string() {
        let mut w = ProtobufWriter::new();
        w.write_string(1, "hello");
        // tag: (1 << 3) | 2 = 0x0A, length: 5, then "hello"
        assert_eq!(w.buf, vec![0x0A, 5, b'h', b'e', b'l', b'l', b'o']);
    }

    #[test]
    fn test_write_string_empty_skipped() {
        let mut w = ProtobufWriter::new();
        w.write_string(1, "");
        assert!(w.buf.is_empty());
    }

    #[test]
    fn test_write_varint_zero_skipped() {
        let mut w = ProtobufWriter::new();
        w.write_varint(1, 0);
        assert!(w.buf.is_empty());
    }

    #[test]
    fn test_write_bool_false_skipped() {
        let mut w = ProtobufWriter::new();
        w.write_bool(1, false);
        assert!(w.buf.is_empty());
    }

    #[test]
    fn test_write_fixed64() {
        let mut w = ProtobufWriter::new();
        w.write_fixed64(7, 1_000_000_000);
        // tag: (7 << 3) | 1 = 0x39
        assert_eq!(w.buf.len(), 9); // 1 byte tag + 8 bytes value
        assert_eq!(w.buf[0], 0x39);
        let val = u64::from_le_bytes(w.buf[1..9].try_into().unwrap());
        assert_eq!(val, 1_000_000_000);
    }

    #[test]
    fn test_write_message() {
        let mut inner = ProtobufWriter::new();
        inner.write_string(1, "hi");
        let inner_len = inner.buf.len();

        let mut outer = ProtobufWriter::new();
        outer.write_message(2, &inner);
        // tag: (2 << 3) | 2 = 0x12, length, inner bytes
        assert_eq!(outer.buf[0], 0x12);
        assert_eq!(outer.buf[1], inner_len as u8);
    }

    #[test]
    fn test_encode_traces_roundtrip() {
        let export = OtlpTraceExport {
            resource_spans: vec![OtlpResourceSpans {
                resource: OtlpResource {
                    attributes: vec![OtlpKeyValue {
                        key: "service.name".to_string(),
                        value: OtlpAnyValue::string("test"),
                    }],
                },
                scope_spans: vec![OtlpScopeSpans {
                    scope: OtlpScope {
                        name: "test-scope".to_string(),
                        version: "1.0".to_string(),
                    },
                    spans: vec![OtlpSpan {
                        trace_id: "0af7651916cd43dd8448eb211c80319c".to_string(),
                        span_id: "00f067aa0ba902b7".to_string(),
                        parent_span_id: None,
                        name: "test-span".to_string(),
                        kind: 1,
                        start_time_unix_nano: "1000000000".to_string(),
                        end_time_unix_nano: "2000000000".to_string(),
                        attributes: vec![],
                        events: vec![],
                        status: OtlpSpanStatus {
                            code: 1,
                            message: None,
                        },
                    }],
                }],
            }],
        };
        let bytes = encode_traces(&export);
        assert!(!bytes.is_empty());
        // Verify it starts with a valid protobuf tag for field 1, wire type 2
        assert_eq!(bytes[0], 0x0A);
    }

    #[test]
    fn test_encode_metrics_roundtrip() {
        let export = OtlpMetricExport {
            resource_metrics: vec![OtlpResourceMetrics {
                resource: OtlpResource {
                    attributes: vec![OtlpKeyValue {
                        key: "service.name".to_string(),
                        value: OtlpAnyValue::string("test"),
                    }],
                },
                scope_metrics: vec![OtlpScopeMetrics {
                    scope: OtlpScope {
                        name: "test-scope".to_string(),
                        version: "1.0".to_string(),
                    },
                    metrics: vec![OtlpMetric {
                        name: "test.counter".to_string(),
                        description: "A test counter".to_string(),
                        unit: "{count}".to_string(),
                        sum: Some(OtlpSum {
                            data_points: vec![OtlpNumberDataPoint {
                                attributes: vec![],
                                start_time_unix_nano: "1000000000".to_string(),
                                time_unix_nano: "2000000000".to_string(),
                                as_int: Some(42),
                                as_double: None,
                            }],
                            aggregation_temporality: 1,
                            is_monotonic: true,
                        }),
                        histogram: None,
                    }],
                }],
            }],
        };
        let bytes = encode_metrics(&export);
        assert!(!bytes.is_empty());
        assert_eq!(bytes[0], 0x0A);
    }

    #[test]
    fn test_encode_logs_roundtrip() {
        let export = OtlpLogExport {
            resource_logs: vec![OtlpResourceLogs {
                resource: OtlpResource {
                    attributes: vec![OtlpKeyValue {
                        key: "service.name".to_string(),
                        value: OtlpAnyValue::string("test"),
                    }],
                },
                scope_logs: vec![OtlpScopeLogs {
                    scope: OtlpScope {
                        name: "test-scope".to_string(),
                        version: "1.0".to_string(),
                    },
                    log_records: vec![OtlpLogRecord {
                        time_unix_nano: "1000000000".to_string(),
                        severity_number: 9,
                        severity_text: "INFO".to_string(),
                        body: OtlpAnyValue::string("Test log message"),
                        attributes: vec![],
                        trace_id: Some("0af7651916cd43dd8448eb211c80319c".to_string()),
                        span_id: Some("00f067aa0ba902b7".to_string()),
                    }],
                }],
            }],
        };
        let bytes = encode_logs(&export);
        assert!(!bytes.is_empty());
        assert_eq!(bytes[0], 0x0A);
    }

    #[test]
    fn test_encode_any_value_oneof_zero_values() {
        // int_value = 0 should still be written (it's a oneof)
        let v = OtlpAnyValue::int(0);
        let w = encode_any_value(&v);
        assert!(!w.buf.is_empty());

        // bool_value = false should still be written (it's a oneof)
        let v = OtlpAnyValue::bool(false);
        let w = encode_any_value(&v);
        assert!(!w.buf.is_empty());

        // double_value = 0.0 should still be written (it's a oneof)
        let v = OtlpAnyValue::double(0.0);
        let w = encode_any_value(&v);
        assert!(!w.buf.is_empty());
    }

    #[test]
    fn test_encode_span_with_events_and_parent() {
        let span = OtlpSpan {
            trace_id: "abcdef0123456789abcdef0123456789".to_string(),
            span_id: "0123456789abcdef".to_string(),
            parent_span_id: Some("fedcba9876543210".to_string()),
            name: "child-span".to_string(),
            kind: 3,
            start_time_unix_nano: "5000000000".to_string(),
            end_time_unix_nano: "6000000000".to_string(),
            attributes: vec![OtlpKeyValue {
                key: "http.method".to_string(),
                value: OtlpAnyValue::string("GET"),
            }],
            events: vec![OtlpSpanEvent {
                name: "exception".to_string(),
                time_unix_nano: "5500000000".to_string(),
                attributes: vec![OtlpKeyValue {
                    key: "exception.message".to_string(),
                    value: OtlpAnyValue::string("something broke"),
                }],
            }],
            status: OtlpSpanStatus {
                code: 2,
                message: Some("Error occurred".to_string()),
            },
        };
        let w = encode_span(&span);
        assert!(!w.buf.is_empty());
    }

    #[test]
    fn test_encode_histogram_metric() {
        let export = OtlpMetricExport {
            resource_metrics: vec![OtlpResourceMetrics {
                resource: OtlpResource {
                    attributes: vec![],
                },
                scope_metrics: vec![OtlpScopeMetrics {
                    scope: OtlpScope {
                        name: "test".to_string(),
                        version: "1.0".to_string(),
                    },
                    metrics: vec![OtlpMetric {
                        name: "test.histogram".to_string(),
                        description: "desc".to_string(),
                        unit: "ms".to_string(),
                        sum: None,
                        histogram: Some(OtlpHistogram {
                            data_points: vec![OtlpHistogramDataPoint {
                                attributes: vec![],
                                start_time_unix_nano: "100".to_string(),
                                time_unix_nano: "200".to_string(),
                                count: 10,
                                sum: 500.0,
                                min: 10.0,
                                max: 100.0,
                            }],
                            aggregation_temporality: 1,
                        }),
                    }],
                }],
            }],
        };
        let bytes = encode_metrics(&export);
        assert!(!bytes.is_empty());
    }
}
