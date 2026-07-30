# Rust Resources

## Knowledge

- [The Rust Programming Language: Defining an Enum](https://doc.rust-lang.org/book/ch06-01-defining-an-enum.html)
  Official introduction to enum variants and data carried by variants. Use for: reading
  domain types such as `Provider` and `SessionState`.
- [The Rust Programming Language: The `match` Control Flow Construct](https://doc.rust-lang.org/book/ch06-02-match.html)
  Official explanation of exhaustive pattern matching. Use for: understanding conversion
  and display methods in `domain.rs`.
- [The Rust Programming Language: Ownership](https://doc.rust-lang.org/book/ch04-00-understanding-ownership.html)
  Official foundation for moves, copies, and borrowing. Use for: understanding method
  receivers such as `self`, `&self`, and `&mut self`.
- [Standard Library: `Option`](https://doc.rust-lang.org/std/option/enum.Option.html)
  Canonical API documentation for optional values. Use for: fields and return values that
  may legitimately be absent.
- [Serde: Using derive](https://serde.rs/derive.html)
  Official Serde guide to generated serialization implementations. Use for: the
  `Serialize` and `Deserialize` derives used by TokenPlay's domain types.
- [Ratatui: `TestBackend`](https://docs.rs/ratatui/0.29.0/ratatui/backend/struct.TestBackend.html)
  Official API reference for rendering terminal widgets into an in-memory buffer. Use
  for: deterministic layout fixtures and terminal-size validation without a live TTY.

## Wisdom (Communities)

- [Rust Users Forum](https://users.rust-lang.org/)
  Moderated community for concrete language, compiler, and design questions. Use for:
  comparing idiomatic solutions after forming a specific question.
