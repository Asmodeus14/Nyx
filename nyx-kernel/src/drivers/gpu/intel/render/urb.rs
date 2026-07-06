// src/drivers/gpu/intel/render/urb.rs
//
// URB (Unified Return Buffer) / push-constant allocation (Phase 5 prerequisite).
//
// The URB is an L3-resident buffer the Vertex Fetch unit writes vertex data into and
// the Thread Dispatcher reads to preload EU thread payloads. It MUST be partitioned
// before the 3D pipeline runs, or the VF has nowhere to store vertices and the
// pipeline stalls. Configured via 3DSTATE_URB_{VS,HS,DS,GS} — all four must be
// programmed together (PRM Vol 2a), plus 3DSTATE_PUSH_CONSTANT_ALLOC_* which must not
// overlap the URB_VS region.
//
// Strategy: start from Mesa's known-good Gen9 single-slice URB partition rather than
// deriving one from scratch (a bad partition is a silent stall). GT1F has 192 KB URB,
// GT2 has 384 KB.
//
// Placeholder at Phase 0.

#![allow(dead_code)]
