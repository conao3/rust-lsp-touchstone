import Assay
import LeanRustGen.Embed.lsp_touchstone.h3bb0016c750440d9.proved

instance (priority := high) : Plausible.Arbitrary String where
  arbitrary := return String.ofList (← Plausible.Gen.listOf Plausible.Arbitrary.arbitrary)

deriving instance Inhabited for proved.Resolution

#assay proved.resolve
#assay proved.offset
