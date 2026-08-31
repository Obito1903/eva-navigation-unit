// Frosted-glass post-process for the wireframe background.
//
// A direct port of the old GL `FROST_FRAGMENT_SHADER`: a 5x5 Gaussian-weighted
// blur that diffuses the sharp wireframe, then a lift toward a cool tint
// proportional to local brightness plus a faint base haze, so dark areas read
// as glass rather than void.

#import bevy_core_pipeline::fullscreen_vertex_shader::FullscreenVertexOutput

struct FrostSettings {
    tint: vec3<f32>,
    // Blur spread in texels.
    radius: f32,
    // How far bright areas are pulled toward `tint`.
    strength: f32,
    // Constant cool haze added everywhere.
    haze: f32,
    _padding: vec2<f32>,
}

@group(0) @binding(0) var screen_texture: texture_2d<f32>;
@group(0) @binding(1) var texture_sampler: sampler;
@group(0) @binding(2) var<uniform> settings: FrostSettings;

@fragment
fn fragment(in: FullscreenVertexOutput) -> @location(0) vec4<f32> {
    let texel = 1.0 / vec2<f32>(textureDimensions(screen_texture));

    var sum = vec3<f32>(0.0);
    var total = 0.0;
    for (var x = -2; x <= 2; x++) {
        for (var y = -2; y <= 2; y++) {
            let offset = vec2<f32>(f32(x), f32(y)) * texel * settings.radius;
            let weight = 1.0 / (1.0 + f32(x * x + y * y));
            sum += textureSample(screen_texture, texture_sampler, in.uv + offset).rgb * weight;
            total += weight;
        }
    }
    let blurred = sum / total;

    // This runs before tonemapping, so luma can exceed 1.0 — clamp it before
    // using it as a mix factor.
    let luma = saturate(max(max(blurred.r, blurred.g), blurred.b));
    let frosted = mix(blurred, settings.tint, luma * settings.strength)
        + settings.tint * settings.haze;

    return vec4<f32>(frosted, 1.0);
}
