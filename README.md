# zmate

This is a fork based on zellij version 0.39.1 that allows terminal sharing using ssh.
## This project is in the early stages of development

## Quick Start
1. Build
```bash
cargo xtask build
```
2. Run the following command to start the ssh server
```bash
./target/debug/zellij ssh
```
1. Run the following command in another terminal to connect to the ssh server
```bash
ssh 127.0.0.1 -p 6222
```