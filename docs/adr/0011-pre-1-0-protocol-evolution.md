# ADR 0011: Pre-1.0 Protocol Evolution

Ultty's native protocol is allowed to evolve and break wire compatibility before a 1.0 stability milestone, but only with explicit protocol version or capability negotiation. Unsupported native capabilities should fail clearly or degrade explicitly, and breaking changes should be reserved for improvements to the semantic model rather than incidental cleanup.
