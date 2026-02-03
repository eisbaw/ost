# Transport-CC - Transport-wide Congestion Control

RTCP feedback extension providing per-packet arrival times for bandwidth estimation. More accurate than REMB.

## How It Works

1. Sender adds sequence number to each packet
2. Receiver records arrival time of each packet
3. Receiver sends feedback with arrival times
4. Sender calculates delay variations and estimates bandwidth

## SDP

```
a=extmap:5 http://www.ietf.org/id/draft-holmer-rmcat-transport-wide-cc-extensions-01
```

## Related Terms

- [BWE](bwe.md)
- [GCC](gcc.md)
- [RTCP](rtcp.md)
