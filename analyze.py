import re
from datetime import datetime

pubsub_pattern = r"\[(\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}\.\d{3}Z).*pubsub: (\d+)"
shreds_pattern = r"\[(\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}\.\d{3}Z).*algo: (\d+)"

pubsub_transactions = []
shreds_transactions = []


# Function to parse the timestamp
def parse_timestamp(ts_string):
    return datetime.strptime(ts_string, "%Y-%m-%dT%H:%M:%S.%fZ")


# Process the log streams
for line in log_streams.split("\n"):
    pubsub_match = re.search(pubsub_pattern, line)
    shreds_match = re.search(shreds_pattern, line)

    if pubsub_match:
        timestamp, transaction_id = pubsub_match.groups()
        pubsub_transactions.append((parse_timestamp(timestamp), int(transaction_id)))
    elif shreds_match:
        timestamp, transaction_id = shreds_match.groups()
        shreds_transactions.append((parse_timestamp(timestamp), int(transaction_id)))

# Count transactions
pubsub_count = len(pubsub_transactions)
shreds_count = len(shreds_transactions)

print(f"Number of pubsub transactions: {pubsub_count}")
print(f"Number of shreds transactions: {shreds_count}")

# Compare timing if both types of transactions exist
if pubsub_count > 0 and shreds_count > 0:
    pubsub_start = min(t[0] for t in pubsub_transactions)
    pubsub_end = max(t[0] for t in pubsub_transactions)
    shreds_start = min(t[0] for t in shreds_transactions)
    shreds_end = max(t[0] for t in shreds_transactions)

    pubsub_duration = (pubsub_end - pubsub_start).total_seconds()
    shreds_duration = (shreds_end - shreds_start).total_seconds()

    print(f"Pubsub duration: {pubsub_duration:.3f} seconds")
    print(f"Shreds duration: {shreds_duration:.3f} seconds")

    if shreds_duration < pubsub_duration:
        print("Shreds was faster")
    elif shreds_duration > pubsub_duration:
        print("Pubsub was faster")
    else:
        print("Pubsub and Shreds had the same duration")
else:
    print("Not enough data to compare timing")
