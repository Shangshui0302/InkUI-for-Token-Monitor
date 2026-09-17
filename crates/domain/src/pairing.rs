use hmac::{Hmac, Mac};
use sha2::{Digest, Sha256};
use spake2::{Ed25519Group, Identity, Password, Spake2};

const CONTEXT_DOMAIN: &[u8] = b"nekodex/pairing-context/v1";
const CONFIRM_DOMAIN: &[u8] = b"nekodex/pairing-confirm/v1";
const KEY_DOMAIN: &[u8] = b"nekodex/pairing-session-key/v1";

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PairingContext {
    pub group_id: Vec<u8>,
    pub group_name: Vec<u8>,
    pub inviter_id: Vec<u8>,
    pub joiner_id: Vec<u8>,
    pub inviter_nonce: [u8; 32],
    pub joiner_nonce: [u8; 32],
}

pub struct PairingHandshake {
    state: Spake2<Ed25519Group>,
    context_hash: [u8; 32],
    outbound: Vec<u8>,
    role: Role,
}

#[derive(Clone, Copy)]
enum Role {
    Inviter,
    Joiner,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PairingResult {
    pub session_key: [u8; 32],
    pub confirmation: [u8; 32],
    transcript_hash: [u8; 32],
}

impl PairingContext {
    pub fn start_inviter(&self, short_code: &[u8]) -> (PairingHandshake, Vec<u8>) {
        self.start(short_code, Role::Inviter)
    }

    pub fn start_joiner(&self, short_code: &[u8]) -> (PairingHandshake, Vec<u8>) {
        self.start(short_code, Role::Joiner)
    }

    fn start(&self, short_code: &[u8], role: Role) -> (PairingHandshake, Vec<u8>) {
        let context_hash = self.context_hash();
        let inviter = Identity::new(&identity(b"inviter", &self.inviter_id, &context_hash));
        let joiner = Identity::new(&identity(b"joiner", &self.joiner_id, &context_hash));
        let password = Password::new(short_code);
        let (state, outbound) = match role {
            Role::Inviter => Spake2::<Ed25519Group>::start_a(&password, &inviter, &joiner),
            Role::Joiner => Spake2::<Ed25519Group>::start_b(&password, &inviter, &joiner),
        };
        (
            PairingHandshake {
                state,
                context_hash,
                outbound: outbound.clone(),
                role,
            },
            outbound,
        )
    }

    fn context_hash(&self) -> [u8; 32] {
        let mut digest = Sha256::new();
        digest.update(CONTEXT_DOMAIN);
        update_field(&mut digest, &self.group_id);
        update_field(&mut digest, &self.group_name);
        update_field(&mut digest, &self.inviter_id);
        update_field(&mut digest, &self.joiner_id);
        update_field(&mut digest, &self.inviter_nonce);
        update_field(&mut digest, &self.joiner_nonce);
        digest.finalize().into()
    }
}

impl PairingHandshake {
    pub fn finish(self, peer_message: &[u8]) -> Result<PairingResult, spake2::Error> {
        let shared = self.state.finish(peer_message)?;
        let (inviter_message, joiner_message, role_label) = match self.role {
            Role::Inviter => (&self.outbound[..], peer_message, b"inviter".as_slice()),
            Role::Joiner => (peer_message, &self.outbound[..], b"joiner".as_slice()),
        };
        let transcript = transcript_hash(&self.context_hash, inviter_message, joiner_message);
        let session_key = derive(&shared, KEY_DOMAIN, &transcript);
        let confirmation = derive(
            &session_key,
            CONFIRM_DOMAIN,
            &[transcript.as_slice(), role_label].concat(),
        );
        Ok(PairingResult {
            session_key,
            confirmation,
            transcript_hash: transcript,
        })
    }

    pub fn verify_peer(
        result: &PairingResult,
        peer_confirmation: &[u8],
        peer_role: &'static [u8],
    ) -> bool {
        let expected = derive(
            &result.session_key,
            CONFIRM_DOMAIN,
            &[result.transcript_hash.as_slice(), peer_role].concat(),
        );
        constant_time_eq(&expected, peer_confirmation)
    }
}

fn identity(role: &[u8], device_id: &[u8], context_hash: &[u8; 32]) -> Vec<u8> {
    let mut value = Vec::with_capacity(role.len() + device_id.len() + context_hash.len() + 8);
    value.extend_from_slice(role);
    value.extend_from_slice(&(device_id.len() as u64).to_be_bytes());
    value.extend_from_slice(device_id);
    value.extend_from_slice(context_hash);
    value
}

fn transcript_hash(
    context_hash: &[u8; 32],
    inviter_message: &[u8],
    joiner_message: &[u8],
) -> [u8; 32] {
    let mut digest = Sha256::new();
    digest.update(CONFIRM_DOMAIN);
    update_field(&mut digest, context_hash);
    update_field(&mut digest, inviter_message);
    update_field(&mut digest, joiner_message);
    digest.finalize().into()
}

fn derive(key: &[u8], domain: &[u8], data: &[u8]) -> [u8; 32] {
    let mut mac = Hmac::<Sha256>::new_from_slice(key).expect("HMAC accepts any key length");
    mac.update(domain);
    update_mac_field(&mut mac, data);
    mac.finalize().into_bytes().into()
}

fn update_field(digest: &mut Sha256, value: &[u8]) {
    digest.update((value.len() as u64).to_be_bytes());
    digest.update(value);
}

fn update_mac_field(mac: &mut Hmac<Sha256>, value: &[u8]) {
    mac.update(&(value.len() as u64).to_be_bytes());
    mac.update(value);
}

fn constant_time_eq(left: &[u8], right: &[u8]) -> bool {
    if left.len() != right.len() {
        return false;
    }
    left.iter()
        .zip(right)
        .fold(0_u8, |diff, (a, b)| diff | (a ^ b))
        == 0
}

#[cfg(test)]
mod tests {
    use super::*;

    fn context() -> PairingContext {
        PairingContext {
            group_id: b"group".to_vec(),
            group_name: b"My devices".to_vec(),
            inviter_id: b"device-a".to_vec(),
            joiner_id: b"device-b".to_vec(),
            inviter_nonce: [1; 32],
            joiner_nonce: [2; 32],
        }
    }

    #[test]
    fn same_code_and_context_produce_same_key_without_sending_the_code() {
        let code = b"492817";
        let (inviter, inviter_message) = context().start_inviter(code);
        let (joiner, joiner_message) = context().start_joiner(code);
        assert!(!inviter_message.windows(code.len()).any(|part| part == code));
        assert!(!joiner_message.windows(code.len()).any(|part| part == code));
        let inviter_result = inviter.finish(&joiner_message).unwrap();
        let joiner_result = joiner.finish(&inviter_message).unwrap();
        assert_eq!(inviter_result.session_key, joiner_result.session_key);
        assert_ne!(inviter_result.confirmation, joiner_result.confirmation);
        assert!(PairingHandshake::verify_peer(
            &inviter_result,
            &joiner_result.confirmation,
            b"joiner"
        ));
        assert!(PairingHandshake::verify_peer(
            &joiner_result,
            &inviter_result.confirmation,
            b"inviter"
        ));
    }

    #[test]
    fn group_name_is_authenticated_by_the_pairing_context() {
        let code = b"492817";
        let inviter_context = context();
        let mut joiner_context = context();
        joiner_context.group_name = b"tampered name".to_vec();
        let (inviter, inviter_message) = inviter_context.start_inviter(code);
        let (joiner, joiner_message) = joiner_context.start_joiner(code);
        let inviter_result = inviter.finish(&joiner_message).unwrap();
        let joiner_result = joiner.finish(&inviter_message).unwrap();
        assert!(!PairingHandshake::verify_peer(
            &inviter_result,
            &joiner_result.confirmation,
            b"joiner",
        ));
    }

    #[test]
    fn context_changes_are_bound_into_the_exchange() {
        let code = b"492817";
        let (inviter, inviter_message) = context().start_inviter(code);
        let mut altered = context();
        altered.group_id = b"other-group".to_vec();
        let (joiner, joiner_message) = altered.start_joiner(code);
        let inviter_result = inviter.finish(&joiner_message).unwrap();
        let joiner_result = joiner.finish(&inviter_message).unwrap();
        assert_ne!(inviter_result.session_key, joiner_result.session_key);
        assert!(!PairingHandshake::verify_peer(
            &inviter_result,
            &joiner_result.confirmation,
            b"joiner"
        ));
    }

    #[test]
    fn wrong_codes_do_not_confirm() {
        let (inviter, inviter_message) = context().start_inviter(b"111111");
        let (joiner, joiner_message) = context().start_joiner(b"222222");
        let inviter_result = inviter.finish(&joiner_message).unwrap();
        let joiner_result = joiner.finish(&inviter_message).unwrap();
        assert_ne!(inviter_result.session_key, joiner_result.session_key);
        assert!(!PairingHandshake::verify_peer(
            &inviter_result,
            &joiner_result.confirmation,
            b"joiner"
        ));
    }
}
