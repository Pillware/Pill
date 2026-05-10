struct VertexOutput {
    float4 sv_position                : SV_Position;
    float2 normalized_device_coordinates : TEXCOORD0;  // NDC XY passed to fragment for ray reconstruction
};

// Full-screen triangle: 3 vertices cover NDC [-1,1]² without a vertex buffer.
VertexOutput vs_main(uint vertex_id : SV_VertexID) {
    float2 positions[3] = {
        float2(-1.0, -3.0),
        float2( 3.0,  1.0),
        float2(-1.0,  1.0)
    };
    VertexOutput output;
    output.sv_position = float4(positions[vertex_id], 1.0, 1.0);  // NDC z/w = 1.0 = far plane (LessEqual depth test)
    output.normalized_device_coordinates = positions[vertex_id];
    return output;
}
