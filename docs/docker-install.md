# Docker Compose on Ubuntu 24.04 LTS

Docker Compose v2 ships as a Docker plugin — no separate binary needed.

```bash
# 1. Add Docker's official repo (skip if docker-ce already installed)
sudo apt update
sudo apt install -y ca-certificates curl
sudo install -m 0755 -d /etc/apt/keyrings
sudo curl -fsSL https://download.docker.com/linux/ubuntu/gpg -o /etc/apt/keyrings/docker.asc
echo "deb [arch=$(dpkg --print-architecture) signed-by=/etc/apt/keyrings/docker.asc] \
  https://download.docker.com/linux/ubuntu $(. /etc/os-release && echo "$VERSION_CODENAME") stable" | \
  sudo tee /etc/apt/sources.list.d/docker.list > /dev/null

# 2. Install Docker Engine + Compose plugin
sudo apt update
sudo apt install -y docker-ce docker-ce-cli containerd.io docker-compose-plugin

# 3. Run without sudo
sudo usermod -aG docker $USER
newgrp docker

# 4. Verify
docker compose version
```

The command is `docker compose` (space, not hyphen). The old `docker-compose` binary is deprecated.