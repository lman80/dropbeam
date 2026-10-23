//! Waking a device that can't be dialed right now (an iPhone with DropBeam in
//! the background) so it can ring: a VoIP push through the push relay. Returns
//! true when a wake-up was sent and a redial is worth trying.

use crate::iroh_net::IrohState;

/// Ask `endpoint` to wake up for call `call_id`. No push path → false.
pub async fn wake(_state: &IrohState, _endpoint: &str, _call_id: &str, _video: bool) -> bool {
    false
}
