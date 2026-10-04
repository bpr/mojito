# PROBE (divergence): a method's reference result as the source of an
# implicit view conversion.
#
# `f(p.name_ref())` at a `StringSpan` parameter binds the view's
# `ref [origin]` source to the referent `name_ref` returns. The pin runs it;
# Mojito's checker rejects the conversion's source as no place. The same
# operand of `+=` (`out += p.name_ref()`) fails the same way, while a
# subscript element (`f(xs[0])`) and an explicit `StringSpan(p.name_ref())`
# run.
#
# Observed 2026-10-03 against `Mojo 1.2.0.dev2026092105`:
#   mojo:   4
#   mojito: unsupported feature: reference binding to a non-place expression
#
# When fixed: promote to an `assets/ok` fixture and delete the roadmap entry.
@fieldwise_init
struct P(Copyable, Movable):
    var name: String

    def name_ref(ref self) -> ref [self.name] String:
        return self.name


def f(s: StringSpan) -> Int:
    return s.byte_length()


def main():
    var p = P("wxyz")
    print(f(p.name_ref()))
