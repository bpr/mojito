# PROBE (divergence): a method stores to a field of `self` while a view of
# that field's owned interior is still live.
#
# The pinned Mojo rejects the store: the view's origin is
# `origin_of(self.name)._get_owned_interior["bytes"]`, and the assignment
# invalidates it before `view` is read. Mojito rejects the same store
# through a local (`h.name = …` in `main`, "access to 'h.name' conflicts with
# live reference 'view'") but accepts it through `self` and prints `2`.
#
# Observed 2026-09-25 against `Mojo 1.2.0.dev2026092105 (e9569894)`:
#   mojo:   error: ... note: origin was invalidated here
#   mojito: 2
#
# When fixed: promote to `assets/type_error`.
struct Named(Movable):
    var name: String

    def __init__(out self, var name: String):
        self.name = name^

    def clobber(mut self) -> Int:
        var view = self.name.strip()
        self.name = String("q")
        return view.byte_length()


def main():
    var a = Named(String("  ab  "))
    print(a.clobber())
