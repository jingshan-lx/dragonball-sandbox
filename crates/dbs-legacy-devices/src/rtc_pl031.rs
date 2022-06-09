// Copyright 2022 Alibaba Cloud. All Rights Reserved.
// Copyright 2019 Amazon.com, Inc. or its affiliates. All Rights Reserved.
// SPDX-License-Identifier: Apache-2.0

//! ARM PL031 Real Time Clock
//!
//! This module implements a PL031 Real Time Clock (RTC) that provides to provides long time base counter.
//! This is achieved by generating an interrupt signal after counting for a programmed number of cycles of
//! a real-time clock input.
use std::convert::TryInto;

use dbs_device::{DeviceIoMut, IoAddress};
use dbs_utils::metric::{IncMetric, SharedIncMetric};
use log::warn;
use vm_superio::rtc_pl031::{Rtc, RtcEvents};

/// Metrics specific to the RTC device
#[derive(Default)]
pub struct RTCDeviceMetrics {
    /// Errors triggered while using the RTC device.
    pub error_count: SharedIncMetric,
    /// Number of superfluous read intents on this RTC device.
    pub missed_read_count: SharedIncMetric,
    /// Number of superfluous write intents on this RTC device.
    pub missed_write_count: SharedIncMetric,
}

impl RtcEvents for RTCDeviceMetrics {
    fn invalid_read(&self) {
        self.missed_read_count.inc();
        self.error_count.inc();
    }

    fn invalid_write(&self) {
        self.missed_write_count.inc();
        self.error_count.inc();
    }
}

pub struct RTCDevice {
    pub rtc: Rtc<RTCDeviceMetrics>,
}

impl RTCDevice {
    pub fn new() -> Self {
        let metrics = RTCDeviceMetrics::default();
        Self {
            rtc: Rtc::with_events(metrics),
        }
    }
}

impl DeviceIoMut for RTCDevice {
    fn read(&mut self, _base: IoAddress, offset: IoAddress, data: &mut [u8]) {
        if data.len() == 4 {
            self.rtc
                .read(offset.raw_value() as u16, data.try_into().unwrap())
        } else {
            warn!(
                "Invalid RTC PL031 read: offset {}, data length {}",
                offset.raw_value(),
                data.len()
            );
            self.rtc.events().invalid_read();
        }
    }

    fn write(&mut self, _base: IoAddress, offset: IoAddress, data: &[u8]) {
        if data.len() == 4 {
            self.rtc
                .write(offset.raw_value() as u16, data.try_into().unwrap())
        } else {
            warn!(
                "Invalid RTC PL031 write: offset {}, data length {}",
                offset.raw_value(),
                data.len()
            );
            self.rtc.events().invalid_write();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    impl RTCDevice {
        fn read(&mut self, offset: u64, data: &mut [u8]) {
            DeviceIoMut::read(self, IoAddress::from(0), IoAddress::from(offset), data)
        }

        fn write(&mut self, offset: u64, data: &[u8]) {
            DeviceIoMut::write(self, IoAddress::from(0), IoAddress::from(offset), data)
        }
    }

    #[test]
    fn test_rtc_read_write_and_event() {
        let mut rtc = RTCDevice::new(EventFd::new(libc::EFD_NONBLOCK).unwrap());
        let mut data = [0; 4];

        // Read and write to the MR register.
        LittleEndian::write_u32(&mut data, 123);
        rtc.write(RTCMR, &data);
        rtc.read(RTCMR, &mut data);
        let v = LittleEndian::read_u32(&data[..]);
        assert_eq!(v, 123);

        // Read and write to the LR register.
        let v = dbs_utils::time::get_time_ns(dbs_utils::time::ClockType::Real);
        LittleEndian::write_u32(&mut data, (v / dbs_utils::time::NANOS_PER_SECOND) as u32);
        let previous_now_before = rtc.previous_now;
        rtc.write(RTCLR, &data);

        assert!(rtc.previous_now > previous_now_before);

        rtc.read(RTCLR, &mut data);
        let v_read = LittleEndian::read_u32(&data[..]);
        assert_eq!((v / dbs_utils::time::NANOS_PER_SECOND) as u32, v_read);

        // Read and write to IMSC register.
        // Test with non zero value.
        let non_zero = 1;
        LittleEndian::write_u32(&mut data, non_zero);
        rtc.write(RTCIMSC, &data);
        // The interrupt line should be on.
        assert!(rtc.interrupt_evt.read().unwrap() == 1);
        rtc.read(RTCIMSC, &mut data);
        let v = LittleEndian::read_u32(&data[..]);
        assert_eq!(non_zero & 1, v);

        // Now test with 0.
        LittleEndian::write_u32(&mut data, 0);
        rtc.write(RTCIMSC, &data);
        rtc.read(RTCIMSC, &mut data);
        let v = LittleEndian::read_u32(&data[..]);
        assert_eq!(0, v);

        // Read and write to the ICR register.
        LittleEndian::write_u32(&mut data, 1);
        rtc.write(RTCICR, &data);
        // The interrupt line should be on.
        assert!(rtc.interrupt_evt.read().unwrap() > 1);
        let v_before = LittleEndian::read_u32(&data[..]);
        let no_errors_before = rtc.metrics.error_count.count();
        rtc.read(RTCICR, &mut data);
        let no_errors_after = rtc.metrics.error_count.count();
        let v = LittleEndian::read_u32(&data[..]);
        // ICR is a  write only register. Data received should stay equal to data sent.
        assert_eq!(v, v_before);
        assert_eq!(no_errors_after - no_errors_before, 1);

        // Attempts to turn off the RTC should not go through.
        LittleEndian::write_u32(&mut data, 0);
        rtc.write(RTCCR, &data);
        rtc.read(RTCCR, &mut data);
        let v = LittleEndian::read_u32(&data[..]);
        assert_eq!(v, 1);

        // Attempts to write beyond the writable space. Using here the space used to read
        // the CID and PID from.
        LittleEndian::write_u32(&mut data, 0);
        let no_errors_before = rtc.metrics.error_count.count();
        rtc.write(AMBA_ID_LOW, &data);
        let no_errors_after = rtc.metrics.error_count.count();
        assert_eq!(no_errors_after - no_errors_before, 1);
        // However, reading from the AMBA_ID_LOW should succeed upon read.

        let mut data = [0; 4];
        rtc.read(AMBA_ID_LOW, &mut data);
        let index = AMBA_ID_LOW + 3;
        assert_eq!(data[0], PL031_ID[((index - AMBA_ID_LOW) >> 2) as usize]);
    }
}
