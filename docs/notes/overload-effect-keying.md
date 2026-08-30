# Overloaded method transfer-effect identity

Cross-call transfer effects are semantic properties of one callable, not of a
source method name. An overload that stores a borrowed value into `self` may
carry a loan-transfer effect while a same-name consuming overload carries none.

Method body checking therefore commits effects under the same
signature-qualified symbol produced by `method_lowered_name`, and method-call
replay uses the checker-selected lowered target. Unique methods retain their
readable `Struct.method` key. Abstract trait dispatch has no selected concrete
body, so it conservatively unions the keys for every overload of every
conforming implementation.

This distinction matters for `List.extend`: its borrowing `Span` overload
transfers the source loan into the destination, while `extend(var other: Self)`
moves elements and leaves no such loan. Sharing the bare `List.extend` key made
the consuming call replay the borrowing effect at the same point it moved its
argument, producing a false use-after-move error.
