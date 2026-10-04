# expect: use of invalidated interior reference 'view' to 'self.name["bytes"]'
# A view of a field's owned interior (`self.name.strip()`) borrows
# `self.name["bytes"]`, so a store over the field in the same `mut self`
# method invalidates it and the view's next use is rejected, as it is through
# a local or a `mut` parameter.
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
