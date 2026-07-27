use std::collections::VecDeque;

use super::MAX_STDERR_BYTES;

#[derive(Default)]
pub(super) struct BoundedDiagnostics {
    pub(super) bytes: VecDeque<u8>,
}

impl BoundedDiagnostics {
    pub(super) fn append(&mut self, chunk: &[u8]) {
        for byte in chunk {
            if self.bytes.len() == MAX_STDERR_BYTES {
                self.bytes.pop_front();
            }
            self.bytes.push_back(*byte);
        }
    }

    pub(super) fn is_empty(&self) -> bool {
        self.bytes.is_empty()
    }
}

pub(super) struct StreamingSecretRedactor {
    pending: Vec<u8>,
    secrets: Vec<Vec<u8>>,
    overlap: usize,
}

impl StreamingSecretRedactor {
    pub(super) fn new(mut secrets: Vec<Vec<u8>>) -> Self {
        secrets.sort_by(|left, right| right.len().cmp(&left.len()).then_with(|| left.cmp(right)));
        secrets.dedup();
        let overlap = secrets
            .iter()
            .map(Vec::len)
            .max()
            .unwrap_or(1)
            .saturating_sub(1);
        Self {
            pending: Vec::new(),
            secrets,
            overlap,
        }
    }

    pub(super) fn push(&mut self, chunk: &[u8]) -> Vec<u8> {
        self.pending.extend_from_slice(chunk);
        redact_diagnostic_bytes(&mut self.pending, &self.secrets);
        let emit = self.pending.len().saturating_sub(self.overlap);
        self.pending.drain(..emit).collect()
    }

    pub(super) fn finish(mut self) -> Vec<u8> {
        redact_diagnostic_bytes(&mut self.pending, &self.secrets);
        self.pending
    }
}

fn redact_diagnostic_bytes(output: &mut [u8], secrets: &[Vec<u8>]) {
    for secret in secrets {
        if secret.is_empty() || secret.len() > output.len() {
            continue;
        }
        let mut offset = 0;
        while let Some(index) = output[offset..]
            .windows(secret.len())
            .position(|window| window == secret)
        {
            let start = offset + index;
            output[start..start + secret.len()].fill(b'*');
            offset = start + secret.len();
        }
    }
}
