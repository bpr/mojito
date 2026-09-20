# PROBE (defect, silent wrong answer): a reference returned through an origin
# binder does not keep the argument it names alive.
#
# `pick` returns its `ref[o]` parameter as `ref[o]`. At the argument's last
# use the pinned Mojo keeps `w` alive until the returned reference is read;
# Mojito destroys `w` first, and the read yields `None`.
#
# Observed 2026-09-20 against `Mojo 1.1.0.dev2026082605 (dd957314)`:
#   mojo:   w / w
#   mojito: w / None
#
# When fixed: promote to `assets/ok`, and drop the trailing `print(w)` that
# `assets/ok/template_method_origin_parameter.mojo` carries to stay clear of
# this.
struct Shelf[T: ImplicitlyCopyable & Deinitable]:
    var count: Int

    def __init__(out self):
        self.count = 0

    def pick[o: Origin](self, ref[o] x: Self.T) -> ref[o] Self.T:
        return x


def main():
    var words = Shelf[String]()
    var w = String("w")
    print(words.pick(w))
    print(words.pick(w))
