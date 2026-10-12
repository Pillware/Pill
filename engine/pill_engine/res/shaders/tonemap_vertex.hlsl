struct VertexOutput {
    float4 sv_position         : SV_Position;
    float2 texture_coordinates : TEXCOORD0;
};

VertexOutput vs_main(uint vertex_id : SV_VertexID) {
    float2 positions[3] = {
        float2(-1.0, -3.0),
        float2( 3.0,  1.0),
        float2(-1.0,  1.0)
    };
    VertexOutput output;
    output.sv_position         = float4(positions[vertex_id], 0.0, 1.0);
    output.texture_coordinates = float2(positions[vertex_id].x * 0.5 + 0.5, -positions[vertex_id].y * 0.5 + 0.5);
    return output;
}
