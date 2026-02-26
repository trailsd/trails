# cd ~/src/trails/client-python
# source .venv/bin/activate
# python3 -c "
from trails import TrailsClient, TrailsConfig
import uuid, time

config = TrailsConfig(
    app_id=str(uuid.uuid4()),
    app_name='e2e-test',
    server_ep='ws://localhost:8443/ws',
)
import os; os.environ['TRAILS_INFO'] = config.encode()

g = TrailsClient.init()
time.sleep(1)
print(f'connected: {g.is_connected}')
g.status({'phase': 'processing', 'progress': 0.5})
g.result({'rows': 100000})
time.sleep(1)
g.shutdown()
print('done')
# "