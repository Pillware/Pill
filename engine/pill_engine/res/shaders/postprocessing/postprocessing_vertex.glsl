#version 450

// Vertex shader for fullscreen triangle
layout(location = 0) out vec2 out_texture_coordinates;

void main() {
    // Generate a fullscreen triangle
    vec2 positions[3] = vec2[](
        vec2(-1.0, -1.0),
        vec2( 3.0, -1.0),
        vec2(-1.0,  3.0)
    );
    
    gl_Position = vec4(positions[gl_VertexIndex], 0.0, 1.0);
    
    // Fix the texture coordinate calculation to prevent horizontal flipping
    vec2 position = positions[gl_VertexIndex];
    out_texture_coordinates = vec2(position.x * 0.5 + 0.5, position.y * -0.5 + 0.5);
}