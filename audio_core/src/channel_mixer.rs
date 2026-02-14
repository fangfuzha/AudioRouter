//! Audio channel mixing utilities.
//!
//! This module provides functionality to mix and route audio channels based on
//! different speaker configurations and mixing modes. It supports converting
//! stereo input to various multi-channel output formats.

use config::ChannelMixMode;

/// Audio channel mixer that processes stereo input samples according to the selected mixing mode.
#[derive(Debug, Clone)]
pub struct ChannelMixer {
    mode: ChannelMixMode,
}

impl ChannelMixer {
    /// Creates a new channel mixer with the specified mixing mode.
    pub fn new(mode: ChannelMixMode) -> Self {
        Self { mode }
    }

    /// Updates the mixing mode.
    pub fn set_mode(&mut self, mode: ChannelMixMode) {
        self.mode = mode;
    }

    /// Processes a buffer of interleaved stereo f32 samples and returns the mixed output.
    ///
    /// # Arguments
    /// * `input` - Interleaved stereo samples (L, R, L, R, ...)
    /// * `output_channels` - Number of output channels to mix to
    ///
    /// # Returns
    /// A vector of mixed samples with `output_channels` channels
    pub fn process_samples(&self, input: &[f32], output_channels: usize) -> Vec<f32> {
        if input.len() % 2 != 0 {
            // Input should be stereo (even number of samples)
            return Vec::new();
        }

        let num_frames = input.len() / 2;
        let mut output = Vec::with_capacity(num_frames * output_channels);

        for frame in 0..num_frames {
            let left = input[frame * 2];
            let right = input[frame * 2 + 1];

            let mixed_channels = self.mix_channels(left, right, output_channels);
            output.extend(mixed_channels);
        }

        output
    }

    /// Mixes a single stereo frame (left, right) to the specified number of output channels.
    fn mix_channels(&self, left: f32, right: f32, output_channels: usize) -> Vec<f32> {
        match self.mode {
            ChannelMixMode::Stereo => {
                // For stereo mode, if output has 2+ channels, copy L/R to first two channels
                // and fill remaining with silence or L/R depending on context
                let mut channels = vec![0.0; output_channels];
                if output_channels >= 1 {
                    channels[0] = left;
                }
                if output_channels >= 2 {
                    channels[1] = right;
                }
                // Additional channels remain 0.0
                channels
            }
            ChannelMixMode::Left => {
                // Left channel to all outputs
                vec![left; output_channels]
            }
            ChannelMixMode::Right => {
                // Right channel to all outputs
                vec![right; output_channels]
            }
            ChannelMixMode::Center => {
                // Mono mix (L+R)/2 to all outputs
                let mono = (left + right) * 0.5;
                vec![mono; output_channels]
            }
            ChannelMixMode::FrontLeft => {
                // Same as Left
                vec![left; output_channels]
            }
            ChannelMixMode::FrontRight => {
                // Same as Right
                vec![right; output_channels]
            }
            ChannelMixMode::BackLeft => {
                // Left channel at 85% volume to all outputs
                vec![left * 0.85; output_channels]
            }
            ChannelMixMode::BackRight => {
                // Right channel at 85% volume to all outputs
                vec![right * 0.85; output_channels]
            }
            ChannelMixMode::BackSurround => {
                // Mono mix to all outputs
                let mono = (left + right) * 0.5;
                vec![mono; output_channels]
            }
            ChannelMixMode::Subwoofer => {
                // Mono mix with 1.3x boost to all outputs
                let mono = (left + right) * 0.5 * 1.3;
                vec![mono; output_channels]
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_stereo_mode() {
        let mixer = ChannelMixer::new(ChannelMixMode::Stereo);
        let input = vec![1.0, 0.5, 0.8, 0.2]; // Two stereo frames
        let output = mixer.process_samples(&input, 2);
        assert_eq!(output, vec![1.0, 0.5, 0.8, 0.2]);
    }

    #[test]
    fn test_left_mode() {
        let mixer = ChannelMixer::new(ChannelMixMode::Left);
        let input = vec![1.0, 0.5]; // One stereo frame
        let output = mixer.process_samples(&input, 4); // 4 output channels
        assert_eq!(output, vec![1.0, 1.0, 1.0, 1.0]);
    }

    #[test]
    fn test_center_mode() {
        let mixer = ChannelMixer::new(ChannelMixMode::Center);
        let input = vec![1.0, 0.5]; // One stereo frame
        let output = mixer.process_samples(&input, 2);
        assert_eq!(output, vec![0.75, 0.75]); // (1.0 + 0.5) / 2 = 0.75
    }

    #[test]
    fn test_subwoofer_mode() {
        let mixer = ChannelMixer::new(ChannelMixMode::Subwoofer);
        let input = vec![1.0, 0.5]; // One stereo frame
        let output = mixer.process_samples(&input, 1);
        assert_eq!(output, vec![0.75 * 1.3]); // (1.0 + 0.5) / 2 * 1.3 = 0.975
    }
}
