# expect: '@always_inline("builtin")' only supports calls to other '@always_inline("builtin")' functions
# A builtin body may call only builtin callables, a static method called
# through `Self` included.
struct G[k: Int]:
    @staticmethod
    def ok(n: Int) -> Int:
        return n

    @always_inline("builtin")
    @staticmethod
    def bad(n: Int) -> Int:
        return Self.ok(n) + Self.k


def main():
    print(1)
