# shreds

use shredstream to perform aribtrage (simple idea, super fucking hard to
implement well)

## TODOs

- safe math sometimes fails with overflow when calculating swap amount, generally will have to
  do something to check if a given transaction can be successful
- ```
  1724687831476 INFO [shreds::listener] metrics: "{\n \"fec_set_failure_count\": 2,\n
    \"fec_set_success_count\": 2836,\n \"fec_sets_remaining\": 1756,\n \"fec_sets_summary\": {\n
    \"incomplete_count\": 1754,\n \"total_count\": 1756\n },\n \"total_collected_coding\":
    102021,\n \"total_collected_data\": 106572,\n \"total_processed_data\": 100661\n}"
  ```
  ^ a lot of the fec sets are hanging, those receive 0 data shreds and only a few coding shreds TODO check if it maybe is fec sets that have already been processed that continue to receive coding shreds
- take volume into account when calculating profit and best size (flash loans might be an
  option)
- there is missing data, likely due to an error somewhere, could be the coding shreds that are
  to be used
- it might be useful to receive a single data tick and inspect on how the shreds are forwarded
  technically, shreds could be used to maintain ledger altogether, the only thing that is needed
- pool calculation might be a bit off, some of the operations are unsupported too
  - the account keys in `update_pool_state_swap` matter, swap base in can be
    with a flipped user account source
    and destination and then it swaps the token in and out
  * when swapping PC2Coin it flips, this might not matter as much as the accounts
- orca is yet to be implememnted, this is to be done after raydium is working

* this bug:

```

1724789637115 ERROR [shreds::shred] Failed to deserialize entry 97/130: io error: failed to fill whole buffer
1724789637115 ERROR [shreds::shred] Failed to deserialize entry 98/130: io error: failed to fill whole buffer
1724789637115 ERROR [shreds::shred] Failed to deserialize entry 99/130: io error: failed to fill whole buffer
1724789637115 ERROR [shreds::shred] Failed to deserialize entry 100/130: io error: failed to fill whole buffer
1724789637115 ERROR [shreds::shred] Failed to deserialize entry 101/130: io error: failed to fill whole buffer
1724789637115 ERROR [shreds::shred] Failed to deserialize entry 102/130: io error: failed to fill whole buffer
1724789637115 ERROR [shreds::shred] Failed to deserialize entry 103/130: io error: failed to fill whole buffer
1724789637115 ERROR [shreds::shred] Failed to deserialize entry 104/130: io error: failed to fill whole buffer
1724789637115 ERROR [shreds::shred] Failed to deserialize entry 105/130: io error: failed to fill whole buffer
1724789637115 ERROR [shreds::shred] Failed to deserialize entry 106/130: io error: failed to fill whole buffer
1724789637115 ERROR [shreds::shred] Failed to deserialize entry 107/130: io error: failed to fill whole buffer
1724789637115 ERROR [shreds::shred] Failed to deserialize entry 108/130: io error: failed to fill whole buffer
1724789637115 ERROR [shreds::shred] Failed to deserialize entry 109/130: io error: failed to fill whole buffer
1724789637115 ERROR [shreds::shred] Failed to deserialize entry 110/130: io error: failed to fill whole buffer
1724789637115 ERROR [shreds::shred] Failed to deserialize entry 111/130: io error: failed to fill whole buffer
1724789637115 ERROR [shreds::shred] Failed to deserialize entry 112/130: io error: failed to fill whole buffer
1724789637115 ERROR [shreds::shred] Failed to deserialize entry 113/130: io error: failed to fill whole buffer
1724789637115 ERROR [shreds::shred] Failed to deserialize entry 114/130: io error: failed to fill whole buffer
1724789637115 ERROR [shreds::shred] Failed to deserialize entry 115/130: io error: failed to fill whole buffer
1724789637115 ERROR [shreds::shred] Failed to deserialize entry 116/130: io error: failed to fill whole buffer
1724789637115 ERROR [shreds::shred] Failed to deserialize entry 117/130: io error: failed to fill whole buffer
1724789637115 ERROR [shreds::shred] Failed to deserialize entry 118/130: io error: failed to fill whole buffer
1724789637115 ERROR [shreds::shred] Failed to deserialize entry 119/130: io error: failed to fill whole buffer
1724789637115 ERROR [shreds::shred] Failed to deserialize entry 120/130: io error: failed to fill whole buffer
1724789637115 ERROR [shreds::shred] Failed to deserialize entry 121/130: io error: failed to fill whole buffer

```

## In the future

- in the algo, ensure that ATAs are already created, this saves some ixs

```

```
