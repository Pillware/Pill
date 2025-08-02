#version 450

// Vertex shader for fullscreen triangle
layout(location = 0) out vec2 tex_coords;

void main() {
    // Generate a fullscreen triangle
    // Vertex 0: (-1, -1)
    // Vertex 1: ( 3, -1) 
    // Vertex 2: (-1,  3)
    vec2 positions[3] = vec2[](
        vec2(-1.0, -1.0),
        vec2( 3.0, -1.0),
        vec2(-1.0,  3.0)
    );
    
    gl_Position = vec4(positions[gl_VertexIndex], 0.0, 1.0);
    
    // Fix the texture coordinate calculation to prevent horizontal flipping
    vec2 pos = positions[gl_VertexIndex];
    tex_coords = vec2(pos.x * 0.5 + 0.5, pos.y * -0.5 + 0.5);
}