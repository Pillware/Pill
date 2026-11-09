// FXAA 3.11 PC quality path, ported from Timothy Lottes' original (NVIDIA, 2011) at
// https://gist.github.com/kosua20/0c506b81b3812ac900048059d2383126 — the copy Lottes
// pointed at himself. Console/360/PS3 paths and the GATHER4 variants are dropped.
//
// Quality preset 39 (FXAA_QUALITY__PS 12, "extreme quality, no dither").
//
// Deviation from the original: the source is the tonemapped image, still linear Rgba16Float
// (the sRGB encode happens when PassLens writes the swapchain). FXAA needs a perceptual luma, so luma is computed as sqrt() of the
// linear luminance — the header's own "gamma 2.0 luma in alpha" recipe, inlined.
// RGB blending therefore happens in linear space rather than on encoded values.

struct FxaaParams {
    float2 rcp_frame;         // 1/width, 1/height of the source — refreshed every frame
    float subpix;             // 0.75  [0..1]        sub-pixel AA amount; lower = sharper
    float edge_threshold;     // 0.166 [0.063..0.333] min local contrast to run the filter
    float edge_threshold_min; // 0.0833 [0.0312..0.0833] trims darks out of the algorithm
};

[[vk::binding(0, 0)]] Texture2D<float4>          t_src;
[[vk::binding(1, 0)]] SamplerState               s_src;
[[vk::binding(2, 0)]] ConstantBuffer<FxaaParams> params;

// Preset 39 search step lengths, in texels along the edge.
static const float QUALITY_P[12] = {
    1.0, 1.0, 1.0, 1.0, 1.0, 1.5, 2.0, 2.0, 2.0, 2.0, 4.0, 8.0
};

float3 fxaa_fetch(float2 pos) {
    return t_src.SampleLevel(s_src, pos, 0.0).rgb;
}

float fxaa_luma(float3 rgb) {
    return sqrt(saturate(dot(rgb, float3(0.299, 0.587, 0.114))));
}

float fxaa_luma_at(float2 pos) {
    return fxaa_luma(fxaa_fetch(pos));
}

float fxaa_luma_off(float2 pos, int2 off) {
    return fxaa_luma(fxaa_fetch(pos + float(off.x) * float2(params.rcp_frame.x, 0.0)
                                    + float(off.y) * float2(0.0, params.rcp_frame.y)));
}

[shader("fragment")]
float4 fs_main(float2 uv : TEXCOORD0) : SV_Target {
    float2 posM = uv;
    float3 rgbM = fxaa_fetch(posM);
    float lumaM = fxaa_luma(rgbM);

    float lumaS = fxaa_luma_off(posM, int2( 0,  1));
    float lumaE = fxaa_luma_off(posM, int2( 1,  0));
    float lumaN = fxaa_luma_off(posM, int2( 0, -1));
    float lumaW = fxaa_luma_off(posM, int2(-1,  0));

    // Local contrast early-out: flat neighbourhoods and darks are left untouched.
    float rangeMax = max(max(lumaN, lumaW), max(lumaE, max(lumaS, lumaM)));
    float rangeMin = min(min(lumaN, lumaW), min(lumaE, min(lumaS, lumaM)));
    float range = rangeMax - rangeMin;
    float rangeMaxClamped = max(params.edge_threshold_min, rangeMax * params.edge_threshold);
    if (range < rangeMaxClamped) {
        return float4(rgbM, 1.0);
    }

    float lumaNW = fxaa_luma_off(posM, int2(-1, -1));
    float lumaSE = fxaa_luma_off(posM, int2( 1,  1));
    float lumaNE = fxaa_luma_off(posM, int2( 1, -1));
    float lumaSW = fxaa_luma_off(posM, int2(-1,  1));

    // Horizontal vs vertical edge: second derivatives across the 3x3 neighbourhood.
    float lumaNS = lumaN + lumaS;
    float lumaWE = lumaW + lumaE;
    float subpixRcpRange = 1.0 / range;
    float subpixNSWE = lumaNS + lumaWE;
    float edgeHorz1 = (-2.0 * lumaM) + lumaNS;
    float edgeVert1 = (-2.0 * lumaM) + lumaWE;

    float lumaNESE = lumaNE + lumaSE;
    float lumaNWNE = lumaNW + lumaNE;
    float edgeHorz2 = (-2.0 * lumaE) + lumaNESE;
    float edgeVert2 = (-2.0 * lumaN) + lumaNWNE;

    float lumaNWSW = lumaNW + lumaSW;
    float lumaSWSE = lumaSW + lumaSE;
    float edgeHorz4 = (abs(edgeHorz1) * 2.0) + abs(edgeHorz2);
    float edgeVert4 = (abs(edgeVert1) * 2.0) + abs(edgeVert2);
    float edgeHorz3 = (-2.0 * lumaW) + lumaNWSW;
    float edgeVert3 = (-2.0 * lumaS) + lumaSWSE;
    float edgeHorz = abs(edgeHorz3) + edgeHorz4;
    float edgeVert = abs(edgeVert3) + edgeVert4;

    float subpixNWSWNESE = lumaNWSW + lumaNESE;
    float lengthSign = params.rcp_frame.x;
    bool horzSpan = edgeHorz >= edgeVert;
    float subpixA = subpixNSWE * 2.0 + subpixNWSWNESE;

    if (!horzSpan) { lumaN = lumaW; }
    if (!horzSpan) { lumaS = lumaE; }
    if (horzSpan)  { lengthSign = params.rcp_frame.y; }
    float subpixB = (subpixA * (1.0 / 12.0)) - lumaM;

    // Pick the steeper of the two gradients across the edge; that side is the one to
    // step half a texel towards.
    float gradientN = lumaN - lumaM;
    float gradientS = lumaS - lumaM;
    float lumaNN = lumaN + lumaM;
    float lumaSS = lumaS + lumaM;
    bool pairN = abs(gradientN) >= abs(gradientS);
    float gradient = max(abs(gradientN), abs(gradientS));
    if (pairN) { lengthSign = -lengthSign; }
    float subpixC = saturate(abs(subpixB) * subpixRcpRange);

    float2 posB = posM;
    float2 offNP;
    offNP.x = (!horzSpan) ? 0.0 : params.rcp_frame.x;
    offNP.y = ( horzSpan) ? 0.0 : params.rcp_frame.y;
    if (!horzSpan) { posB.x += lengthSign * 0.5; }
    if ( horzSpan) { posB.y += lengthSign * 0.5; }

    float2 posN = posB - offNP * QUALITY_P[0];
    float2 posP = posB + offNP * QUALITY_P[0];
    float subpixD = ((-2.0) * subpixC) + 3.0;
    float lumaEndN = fxaa_luma_at(posN);
    float subpixE = subpixC * subpixC;
    float lumaEndP = fxaa_luma_at(posP);

    if (!pairN) { lumaNN = lumaSS; }
    float gradientScaled = gradient * (1.0 / 4.0);
    float lumaMM = lumaM - lumaNN * 0.5;
    float subpixF = subpixD * subpixE;
    bool lumaMLTZero = lumaMM < 0.0;

    lumaEndN -= lumaNN * 0.5;
    lumaEndP -= lumaNN * 0.5;
    bool doneN = abs(lumaEndN) >= gradientScaled;
    bool doneP = abs(lumaEndP) >= gradientScaled;
    if (!doneN) { posN -= offNP * QUALITY_P[1]; }
    if (!doneP) { posP += offNP * QUALITY_P[1]; }

    // End-of-edge search: walk both ways along the edge until the luma stops matching
    // the span, or the preset runs out of steps.
    [unroll]
    for (int i = 2; i < 12; i++) {
        if (doneN && doneP) { break; }
        if (!doneN) { lumaEndN = fxaa_luma_at(posN) - lumaNN * 0.5; }
        if (!doneP) { lumaEndP = fxaa_luma_at(posP) - lumaNN * 0.5; }
        doneN = abs(lumaEndN) >= gradientScaled;
        doneP = abs(lumaEndP) >= gradientScaled;
        if (!doneN) { posN -= offNP * QUALITY_P[i]; }
        if (!doneP) { posP += offNP * QUALITY_P[i]; }
    }

    float dstN = horzSpan ? (posM.x - posN.x) : (posM.y - posN.y);
    float dstP = horzSpan ? (posP.x - posM.x) : (posP.y - posM.y);

    bool goodSpanN = (lumaEndN < 0.0) != lumaMLTZero;
    float spanLength = dstP + dstN;
    bool goodSpanP = (lumaEndP < 0.0) != lumaMLTZero;
    float spanLengthRcp = 1.0 / spanLength;

    bool directionN = dstN < dstP;
    float dst = min(dstN, dstP);
    bool goodSpan = directionN ? goodSpanN : goodSpanP;
    float subpixG = subpixF * subpixF;
    float pixelOffset = (dst * (-spanLengthRcp)) + 0.5;
    float subpixH = subpixG * params.subpix;

    // Edge-based offset when the span closed cleanly, otherwise fall back to the
    // sub-pixel (local contrast) offset alone.
    float pixelOffsetGood = goodSpan ? pixelOffset : 0.0;
    float pixelOffsetSubpix = max(pixelOffsetGood, subpixH);
    if (!horzSpan) { posM.x += pixelOffsetSubpix * lengthSign; }
    if ( horzSpan) { posM.y += pixelOffsetSubpix * lengthSign; }

    return float4(fxaa_fetch(posM), 1.0);
}
