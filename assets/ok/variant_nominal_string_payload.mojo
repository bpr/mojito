# Variant with a nominal String alternative: set stores an independent
# deep-copied payload and a projection value read copies the Copyable
# payload out of the variant's storage.
from std.utils import Variant

def main():
    var value = Variant[Int, String](7)
    value.set[String]("mojo")
    var got: String = value[String]
    print(got)
