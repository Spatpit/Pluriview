// Match RgbaDownsampler's integer sample boundaries and truncating average.
// Recover the original UNORM bytes before averaging, including transparent RGB
// and alpha. Reading the capture surface directly avoids a full-size GPU copy.
Texture2D<float4> source : register(t0);
RWBuffer<uint> target : register(u0);
cbuffer Dimensions : register(b0) {
    uint2 source_size;
    uint2 target_size;
};

[numthreads(8, 8, 1)]
void main(uint3 id : SV_DispatchThreadID) {
    if (any(id.xy >= target_size)) return;
    uint2 first = id.xy * source_size / target_size;
    uint2 last = max((id.xy + 1) * source_size / target_size, first + 1);
    uint4 sum = 0;
    for (uint y = first.y; y < last.y; ++y)
        for (uint x = first.x; x < last.x; ++x)
            sum += uint4(round(source.Load(int3(x, y, 0)) * 255.0));
    uint4 rgba = sum / ((last.x - first.x) * (last.y - first.y));
    target[id.y * target_size.x + id.x] =
        rgba.r | (rgba.g << 8) | (rgba.b << 16) | (rgba.a << 24);
}
