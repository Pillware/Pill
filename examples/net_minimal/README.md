## Net minimal example

This is an example to show how networking works in PillEngine. We have a simple server and multiple clients that can connect/disconnect and reconnect at will and the server will propagate the state to all the other connected clients.

### How to run
Run up to `max_clients` (you can edit that variable to your liking) by simply running the game binary:
`./client`

Then run a server by running:
`cd server`
`cargo run server`

### Controls
Left Arrow - move Left
Right Arrow - move Right
Up Arrow - move towards the camera
Down Arrow - move away from the camera
Left Ctrl - move up
Left Shift - move down
