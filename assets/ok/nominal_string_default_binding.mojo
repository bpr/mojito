# An un-annotated string binding is the nominal String, like current Mojo:
# result APIs and ordering dispatch on it, and the migrated literal
# operations (concatenation, membership, len, printing) keep working.
def main():
    var s = "hé"
    print(s.find("é"))
    print(s.startswith("h"))
    var t = s + "llo"
    print(t)
    print(len(t))
    print("llo" in t)
    print(s < t)
    var picked = "b" if len(t) > 3 else "a"
    print(picked.endswith("b"))
