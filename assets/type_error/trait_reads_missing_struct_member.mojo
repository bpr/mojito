# expect: type 'M' has no associated type 'Y'
# A trait's signature reading a struct member no declaration provides is
# still rejected once the module's structs have registered.


struct M:
    comptime X = Int


trait Tr:
    def f(self, x: M.Y):
        ...


def main():
    print(1)
