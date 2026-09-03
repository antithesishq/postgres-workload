FROM python:3.12-slim

WORKDIR /opt/antigres

COPY requirements.txt /opt/antigres/requirements.txt
RUN pip install --no-cache-dir -r requirements.txt

COPY libvoidstar.so /usr/lib/libvoidstar.so
COPY bank /opt/antigres/bank
COPY durability /opt/antigres/durability
COPY test_client /opt/antigres/test_client
COPY entrypoint /opt/antithesis/test/v1/postgres

RUN mkdir -p /var/lib/antigres \
    && chmod +x /opt/antithesis/test/v1/postgres/*

ENV PYTHONPATH=/opt/antigres
ENV PYTHONUNBUFFERED=1
ENV DURABILITY_LEDGER_PATH=/var/lib/antigres/durability.sqlite3

ENTRYPOINT ["python", "-m", "test_client.entrypoint"]
