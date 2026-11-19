pub trait TransferCore: Send + Sync {

    fn is_full(&self) -> bool;

    fn server_copy_cmd(
        &self,
        src: &str,
        dst: &str,
        is_dir: bool,
        dest_exists: bool,
    ) -> Result<String, &'static str>;

    fn server_copy_ok(&self, exit: i32, output: &str) -> bool;

    fn server_copy_timeout_secs(&self) -> u64;
}

pub struct StubTransferCore;

impl TransferCore for StubTransferCore {
    fn is_full(&self) -> bool {
        false
    }
    fn server_copy_cmd(&self, _: &str, _: &str, _: bool, _: bool) -> Result<String, &'static str> {
        Err("server-copy-gated")
    }
    fn server_copy_ok(&self, _: i32, _: &str) -> bool {
        false
    }
    fn server_copy_timeout_secs(&self) -> u64 {
        0
    }
}

#[cfg(feature = "official")]
pub struct OfficialTransferCore;

#[cfg(feature = "official")]
impl TransferCore for OfficialTransferCore {
    fn is_full(&self) -> bool {
        true
    }
    fn server_copy_cmd(
        &self,
        src: &str,
        dst: &str,
        is_dir: bool,
        dest_exists: bool,
    ) -> Result<String, &'static str> {
        Ok(crate::ifree_core::transfer::server_copy_cmd(
            src,
            dst,
            is_dir,
            dest_exists,
        ))
    }
    fn server_copy_ok(&self, exit: i32, output: &str) -> bool {
        crate::ifree_core::transfer::server_copy_ok(exit, output)
    }
    fn server_copy_timeout_secs(&self) -> u64 {
        crate::ifree_core::transfer::SERVER_COPY_TIMEOUT_SECS
    }
}

pub fn make_transfer_core() -> Box<dyn TransferCore> {
    #[cfg(feature = "official")]
    {
        Box::new(OfficialTransferCore)
    }
    #[cfg(not(feature = "official"))]
    {
        Box::new(StubTransferCore)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stub_gates_server_copy() {
        let core = make_transfer_core();
        if cfg!(feature = "official") {
            assert!(core.is_full());
        } else {

            assert!(!core.is_full());
            assert_eq!(
                core.server_copy_cmd("/a", "/b", true, false),
                Err("server-copy-gated")
            );
            assert!(!core.server_copy_ok(0, "CP_OK"));
        }
    }

    #[cfg(feature = "official")]
    #[test]
    fn official_offers_server_copy() {
        let core = make_transfer_core();
        assert_eq!(core.server_copy_timeout_secs(), 600);
        assert_eq!(
            core.server_copy_cmd("/a", "/b", true, false).unwrap(),
            "cp -a -- '/a' '/b' && echo CP_OK"
        );
        assert!(core.server_copy_ok(0, "CP_OK"));
        assert!(!core.server_copy_ok(0, ""));
    }
}
