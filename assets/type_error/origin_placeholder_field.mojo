# expect: is not concrete
# An origin placeholder in a struct field rejects like an omitted slot:
# storage has no initializer to infer the origin from (pin-attested).
@fieldwise_init
struct Wrap:
    var inner: Span[Int, _]

def main():
    pass
