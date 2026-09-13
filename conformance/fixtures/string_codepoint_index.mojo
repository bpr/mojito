# `s[codepoint=i]` is a `Codepoint` in Mojito and a one-codepoint
# `StringSpan` upstream, so only Mojito binds it to a `Codepoint`.
def main() raises:
    var s = String("gé🙂")
    var g: Codepoint = s[codepoint=0]
    print(Int(g), g.is_ascii())
