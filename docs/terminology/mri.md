# MRI - Microsoft Resource Identifier

**MRI** = **Microsoft Resource Identifier**

Microsoft's internal identifier format for Teams/Skype resources. The format includes a type prefix followed by a GUID or other identifier.

## Format

```
<type-prefix>:<identifier>
```

## Common Prefixes

| Prefix | Resource Type | Example |
|--------|---------------|---------|
| `8:orgid:` | Organizational user (work/school) | `8:orgid:abc123-def456-...` |
| `8:live:` | Consumer/personal account | `8:live:abc123-...` |
| `28:` | Bot/application | `28:cf28171e-fcfd-47e4-...` |
| `19:` | Thread/conversation | `19:meeting_abc123@thread.v2` |

## Examples

- **Bot MRI**: `28:cf28171e-fcfd-47e4-a1d6-79460b0b3ca0` - the `28:` prefix indicates an application/bot
- **User MRI**: `8:orgid:00000000-0000-0000-0000-000000000000` - organizational user identified by OID
- **Thread MRI**: `19:user1_user2@unq.gbl.spaces` - 1:1 conversation thread
