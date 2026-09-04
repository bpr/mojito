# The strip family returns borrowed views: the default set is POSIX
# whitespace, the `chars` form strips by codepoint membership, and
# removeprefix/removesuffix drop one affix when present.
def main():
    var padded = String("  \t mojo \n ")
    print(padded.strip())
    print(padded.strip().byte_length(), padded.lstrip().byte_length(), padded.rstrip().byte_length())
    var wrapped = String("xxhixx")
    print(wrapped.strip("x"), wrapped.lstrip("x"), wrapped.rstrip("x"))
    var accents = String("ééhéé")
    print(accents.strip("é"), accents.strip("é").byte_length())
    var mixed = String("abchelloabc")
    print(mixed.strip("cba"), mixed.strip("xyz"))
    var path = String("prefix_body_suffix")
    print(path.removeprefix("prefix_"), path.removesuffix("_suffix"), path.removeprefix("nope"))
    print(path.removesuffix(""), path.removeprefix("").byte_length())
    var spaces = String("   ")
    print(spaces.strip().byte_length(), Bool(spaces.strip()))
    var view = padded.strip()
    print(view.strip("m"), view.lstrip("mo"), view.rstrip("oj"))
    print(view.removeprefix("mo"), view.removesuffix("jo"))
