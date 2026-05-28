// Copyright (c) 2024 Steven Rosenthal smr@dt3.org
// See LICENSE file in root directory for license terms.

use std::path::PathBuf;
use std::time::Instant;

use clap::Parser;
use image::{GrayImage, ImageReader, Luma};
use log::{info, warn};

use cedar_elements::image_utils::ImageRotator;

/// Test program for rotating an image.
#[derive(Parser, Debug)]
#[command(author, version, about, long_about=None)]
struct Args {
    /// Path of the image file to process.
    #[arg(short, long)]
    input: String,

    /// Rotation angle, degrees.
    #[arg(short, long)]
    angle: f64,

    /// Use the custom optimized algorithm for Pi Zero 2W.
    #[arg(short, long)]
    custom: bool,
}

/// A highly optimized custom rotation function designed for constrained CPUs
/// like the Cortex-A53 (Raspberry Pi Zero 2W).
/// 
/// Algorithmic improvements:
/// 1. Loop Fusion: We skip allocating a full-width rotated intermediate image
///    and skip the secondary crop pass. We only iterate exactly over the pixels
///    that will end up in the final square crop.
/// 2. Fixed-Point Math: Floating-point math is expensive. We precompute the
///    step sizes for rays across the destination image and use 16.16 fixed-point
///    arithmetic (integers) to traverse the source image.
/// 3. Manual Bilinear Interpolation: Uses integer-only weights to avoid any
///    f32 conversions during pixel sampling.
///
/// Note on turbojpeg: turbojpeg is excellent for JPEG encode/decode, but it 
/// cannot perform arbitrary degree rotations (only 90/180/270). Since we need
/// arbitrary affine transforms here, we rely on custom loop logic.
fn custom_rotate_image_and_crop(image: &GrayImage, rotator: &ImageRotator) -> GrayImage {
    let (w, h) = image.dimensions();
    assert!(w >= h, "rotate_image_and_crop requires width >= height, got {}x{}", w, h);
    let square_size = h;

    let mut output = GrayImage::new(square_size, square_size);
    let out_buf = output.as_mut();
    let in_buf = image.as_raw();
    let in_w = w as i32;
    let in_h = h as i32;

    // 16.16 fixed point multiplier
    let f_scale = 65536.0;

    // Calculate how much the source coordinate changes when we step 1 pixel in destination X or Y
    let dx_src_x = (rotator.cos_term * f_scale) as i32;
    let dx_src_y = (rotator.sin_term * f_scale) as i32;
    let dy_src_x = (-rotator.sin_term * f_scale) as i32;
    let dy_src_y = (rotator.cos_term * f_scale) as i32;

    // Find the source coordinate that corresponds to the top-left (0,0) of the output image.
    // The default `transform_from_rotated` gives us the exact floating point starting coordinate.
    let (start_src_x_f, start_src_y_f) = rotator.transform_from_rotated(0.0, 0.0, w, h);
    
    let mut row_src_x = (start_src_x_f * f_scale) as i32;
    let mut row_src_y = (start_src_y_f * f_scale) as i32;

    let mut out_idx = 0;

    for _y in 0..square_size {
        let mut src_x = row_src_x;
        let mut src_y = row_src_y;

        for _x in 0..square_size {
            // Integer pixel coordinates (16.16 shift right by 16)
            let px = src_x >> 16;
            let py = src_y >> 16;
            
            // Fractional parts (0 to 65535)
            let fx = (src_x & 0xFFFF) as u32;
            let fy = (src_y & 0xFFFF) as u32;
            let inv_fx = 65536 - fx;
            let inv_fy = 65536 - fy;

            // Branch predictor friendly Safe Zone check
            let blended = if px >= 0 && px < in_w - 1 && py >= 0 && py < in_h - 1 {
                // FAST PATH: We are guaranteed to be inside the image bounds.
                // We can fetch all 4 adjacent pixels with a single base index calculation
                // and skip all internal bounds checking.
                let base_idx = (py * in_w + px) as usize;
                
                // Using unsafe get_unchecked to guarantee no hidden slice bounds checks 
                // on the inner loop, extracting maximum performance.
                let (p00, p10, p01, p11) = unsafe {
                    (
                        *in_buf.get_unchecked(base_idx) as u32,
                        *in_buf.get_unchecked(base_idx + 1) as u32,
                        *in_buf.get_unchecked(base_idx + in_w as usize) as u32,
                        *in_buf.get_unchecked(base_idx + in_w as usize + 1) as u32,
                    )
                };

                // Factored bilinear math: 6 multiplications instead of 8.
                let top = p00 * inv_fx + p10 * fx;
                let bot = p01 * inv_fx + p11 * fx;
                
                // 64-bit vertical blend to prevent overflow
                ((top as u64 * inv_fy as u64 + bot as u64 * fy as u64) >> 32) as u8
            } else {
                // SLOW PATH (Edges only): Safe fetch helper for bounds checking
                let fetch = |x: i32, y: i32| -> u32 {
                    if x >= 0 && x < in_w && y >= 0 && y < in_h {
                        in_buf[(y * in_w + x) as usize] as u32
                    } else {
                        0 // Default black outside image bounds
                    }
                };

                let p00 = fetch(px, py);
                let p10 = fetch(px + 1, py);
                let p01 = fetch(px, py + 1);
                let p11 = fetch(px + 1, py + 1);

                let top = p00 * inv_fx + p10 * fx;
                let bot = p01 * inv_fx + p11 * fx;
                ((top as u64 * inv_fy as u64 + bot as u64 * fy as u64) >> 32) as u8
            };
            
            out_buf[out_idx] = blended;
            out_idx += 1;

            // Step forward in X direction
            src_x += dx_src_x;
            src_y += dx_src_y;
        }
        
        // Step forward in Y direction (for the next row)
        row_src_x += dy_src_x;
        row_src_y += dy_src_y;
    }

    output
}

fn main() {
    env_logger::Builder::from_env(
        env_logger::Env::default().default_filter_or("info")).init();
    let args = Args::parse();

    let input = args.input.as_str();
    info!("Processing {}", input);
    let input_path = PathBuf::from(&input);
    let mut output_path = PathBuf::from(".");
    output_path.push(input_path.file_name().unwrap());
    
    // Distinguish output file based on method used
    if args.custom {
        output_path.set_extension("custom.bmp");
    } else {
        output_path.set_extension("bmp");
    }

    let img = match ImageReader::open(&input_path).unwrap().decode() {
        Ok(img) => img,
        Err(e) => {
            warn!("Skipping {:?} due to: {:?}", input_path, e);
            return;
        },
    };
    
    let input_img = img.to_luma8();
    let (width, height) = input_img.dimensions();
    let image_rotator = ImageRotator::new(args.angle);
    
    let rotate_start = Instant::now();
    let output_img = if args.custom {
        info!("Using custom optimized rotation algorithm");
        custom_rotate_image_and_crop(&input_img, &image_rotator)
    } else {
        info!("Using default imageproc rotation algorithm");
        image_rotator.rotate_image_and_crop(&input_img)
    };
    let elapsed = rotate_start.elapsed();
    
    info!("Rotated in {:?}", elapsed);

    let (rot_x, rot_y) = image_rotator.transform_to_rotated(0.0, 0.0, width, height);
    info!("Original 0,0 transforms to {:.2},{:.2}", rot_x, rot_y);

    let (x, y) = image_rotator.transform_from_rotated(rot_x, rot_y, width, height);
    info!("Transforms back to {:.2},{:.2}", x, y);

    output_img.save(output_path).unwrap();
}
