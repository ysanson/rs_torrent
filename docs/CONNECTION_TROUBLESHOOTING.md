# BitTorrent Connection Troubleshooting Guide

## Overview

This guide helps diagnose and fix common connection issues when downloading torrents with the rs_torrent BitTorrent client.

## Common Issues and Solutions

### 1. "Peer is not ready to download" Error

#### Symptoms
```
Received bitfield from peer 192.168.1.100:6881
Error downloading from peer 192.168.1.100:6881: Peer is not ready to download
Pipeline: 0 requests sent, 0 responses, 0 timeouts, max depth: 0
Peers: 0 active, 0 total pending requests
```

#### Root Cause
Peers are in a "choking" state and won't allow downloads. This is normal BitTorrent behavior where peers start choking until they decide to unchoke you.

#### Solutions Implemented

1. **Tit-for-Tat Protocol**: Client now sends "unchoke" messages to encourage peers to unchoke us back
2. **Better State Management**: Improved tracking of peer choking/interested states
3. **Non-Blocking Behavior**: "Not ready" errors no longer break the connection loop

#### Code Changes
```rust
// Send unchoke message to encourage peer reciprocity
let unchoke_msg = Message {
    kind: MessageId::Unchoke,
    payload: vec![],
};
stream.write_all(&unchoke_msg.serialize()).await?;
```

### 2. Connection Refused Errors

#### Symptoms
```
Peer worker error: Connection refused (os error 61)
Peer worker error: Connection reset by peer (os error 54)
```

#### Root Cause
- Peers may be behind firewalls or NAT
- Peers may have reached connection limits
- Peers may be offline or unreachable

#### Solutions
1. **Robust Error Handling**: Client continues with other peers when connections fail
2. **Timeout Management**: Configurable connection timeouts
3. **Retry Logic**: Failed connections don't block other peers

### 3. Early EOF and Network Errors

#### Symptoms
```
Peer worker error: early eof
Peer worker error: Network is unreachable (os error 51)
Peer worker error: deadline has elapsed
```

#### Root Cause
- Network connectivity issues
- Peers disconnecting during handshake
- Slow or unstable connections

#### Solutions Implemented
1. **Keep-Alive Messages**: Maintain connection health
2. **Diagnostic Logging**: Better visibility into connection states
3. **Graceful Degradation**: Continue with working peers

## Diagnostic Tools

### 1. Peer Connection Diagnostics

The client now provides detailed diagnostics every 30 seconds:

```
=== PEER CONNECTION DIAGNOSTICS ===
Peer 192.168.1.100:6881:
  State: choking_us=true, we_interested=true, we_choking=false, they_interested=false
  Bitfield: has 1500 pieces available
  Pipeline: 0 pending requests
  Can download: false
  Last activity: Some(45.2s)
===========================
```

### 2. Real-Time Statistics

Monitor connection health in real-time:
```
Progress: 0/2680 pieces (0.0%) | 0/42880 blocks (0.0%)
Pipeline: 0 requests sent, 0 responses, 0 timeouts, max depth: 0
Peers: 5 connected, 0 active, 0 total pending requests
```

### 3. Enhanced Logging

More informative log messages:
```
🔗 Connected to peer 192.168.1.100:6881
✅ Handshake successful with peer - infohash matches
📤 Sent 'interested' to peer 192.168.1.100:6881
📤 Sent 'unchoke' to peer 192.168.1.100:6881
📋 Received bitfield from peer 192.168.1.100:6881 (1500 pieces available)
🚫 Peer 192.168.1.100:6881 not ready: choking_us=true, we_interested=true, we_choking=false, they_interested=false, has_bitfield=true
✅ Peer 192.168.1.100:6881 unchoked us - can now download!
```

## BitTorrent Protocol Flow

### Normal Connection Sequence

1. **TCP Connection**: Establish TCP socket to peer
2. **Handshake**: Exchange protocol handshake with infohash
3. **Initial Messages**: 
   - Send "interested" to indicate we want data
   - Send "unchoke" to encourage reciprocity
4. **Peer Response**:
   - Peer sends bitfield showing available pieces
   - Peer may send "unchoke" to allow downloads
5. **Data Transfer**: Pipeline block requests and receive data

### Common Blocking Points

1. **Handshake Failure**: Infohash mismatch or protocol errors
2. **Choking State**: Peer keeps us choked (most common issue)
3. **No Bitfield**: Peer doesn't advertise available pieces
4. **Network Issues**: Connection drops or timeouts

## Troubleshooting Steps

### Step 1: Check Basic Connectivity

Verify peers are reachable:
```bash
# Test if peer port is open
nc -z 192.168.1.100 6881
telnet 192.168.1.100 6881
```

### Step 2: Examine Torrent Health

Check if torrent has healthy seeders:
- Look at tracker response for active peers
- Verify torrent isn't dead/abandoned
- Check if you have proper announce URLs

### Step 3: Monitor Peer States

Watch for these patterns in diagnostics:
- **All peers choking**: Normal for new connections, should improve over time
- **No bitfields received**: Peers may not have the torrent
- **Constant disconnections**: Network or firewall issues

### Step 4: Adjust Configuration

Consider tweaking these parameters:

```rust
// Connection timeouts
const CONNECTION_TIMEOUT: Duration = Duration::from_secs(10);
const READ_TIMEOUT: Duration = Duration::from_secs(30);
const REQUEST_TIMEOUT: Duration = Duration::from_secs(60);

// Pipeline settings
const MAX_PIPELINE_DEPTH: usize = 10;
```

## Expected Behavior vs Issues

### Expected Behavior
```
🔗 Connected to peer 192.168.1.100:6881
✅ Handshake successful with peer - infohash matches
📤 Sent 'interested' to peer 192.168.1.100:6881
📤 Sent 'unchoke' to peer 192.168.1.100:6881
📋 Received bitfield from peer 192.168.1.100:6881 (1500 pieces available)
✅ Peer 192.168.1.100:6881 unchoked us - can now download!
📤 Requested block: piece 0, offset 0, length 16384 from peer 192.168.1.100:6881 (pipeline: 1)
```

### Problem Indicators
```
# Never gets past "not ready" state
🚫 Peer 192.168.1.100:6881 not ready: choking_us=true, ...

# Immediate disconnections
Peer worker error: Connection refused (os error 61)

# No peer progress
Pipeline: 0 requests sent, 0 responses, 0 timeouts, max depth: 0
Peers: 0 active, 0 total pending requests
```

## Advanced Debugging

### Enable Verbose Logging

To get more detailed information, you can modify the logging levels:

```rust
// Add more debug prints in peer_worker
println!("🔍 Debug: Connection state for {}: {:?}", addr, connection_state);
```

### Monitor Network Traffic

Use network monitoring tools:
```bash
# Monitor BitTorrent traffic
sudo tcpdump -i any port 6881-6889

# Check for specific peer traffic
sudo tcpdump -i any host 192.168.1.100
```

### Check System Resources

Ensure sufficient resources:
```bash
# Check open file descriptors
lsof -p <process_id> | wc -l

# Check network connections
netstat -an | grep :6881
```

## Common Solutions

### 1. Patience is Key
- BitTorrent connections can take time to establish
- Peers may take 30-60 seconds to unchoke new connections
- Some peers never unchoke (this is normal)

### 2. Try Different Torrents
- Test with popular, well-seeded torrents
- Verify the torrent file is valid
- Check tracker announcements are working

### 3. Network Configuration
- Ensure firewall allows outbound connections
- Check if ISP blocks BitTorrent traffic
- Try different network environments

### 4. Client Behavior
- The client now implements proper tit-for-tat
- Keep-alive messages maintain connections
- Better error handling prevents cascade failures

## When to Worry

### Normal Behavior (Don't Worry)
- Some connection failures (50% failure rate is normal)
- Peers staying in choked state initially
- Slow initial progress while peers unchoke

### Problem Indicators (Investigate)
- 100% connection failure rate
- No peers ever unchoke after 5+ minutes
- Constant "deadline has elapsed" errors
- No bitfields received from any peers

## Getting Help

If issues persist after following this guide:

1. **Capture Diagnostics**: Run with full logging for 5+ minutes
2. **Test Network**: Verify basic BitTorrent connectivity
3. **Try Known Good Torrents**: Test with popular, active torrents
4. **Check Torrent Health**: Verify seeders exist and tracker responds

The enhanced logging and diagnostics should provide enough information to identify most connection issues.