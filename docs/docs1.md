# Intial testing
```bash
# 1st terminal
cp ../.env.example .env
cargo run

# 2nd terminal
cd client-python
source .venv/bin/activate
python3 ../scripts/t1.py

# 3rd termianl
docker compose exec postgres psql -U trails -c \
  "SELECT app_id, app_name, status FROM apps;"

# clean db
docker compose exec postgres psql -U trails -c "TRUNCATE crashes, snapshots, messages, apps CASCADE;"
