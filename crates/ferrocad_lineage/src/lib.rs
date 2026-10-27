//! # FerroCAD lineage vocabulary (`ferrocad_lineage`)
//!
//! The kernel-independent vocabulary for provenance: what an operation did to
//! its inputs, in terms any kernel can report, and the stable-name registry
//! derived from it.
//!
//! Everything here is generic over an opaque payload `T`, so this crate is
//! unable to name a geometry type. `ferrocad_geom` instantiates the vocabulary
//! with its `Shape` handle, and tests can run the same algorithms with a
//! `String` payload (see `docs/developer/architecture/lineage-and-naming.md`).
//!
//! It provides:
//!
//! - [`ElementRef`] — one sub-element, by a stable name rather than a positional
//!   index.
//! - [`ElementId`], [`OperandId`] and [`Provenance`] — where an input element came
//!   from: which operand, which kind, which ordinal.
//! - [`History`] — what one operation did to its inputs.
//! - [`ElementMap`] — the stable-name registry carried with a shape.
//! - [`ReverseMap`] — the same relation read backwards: for an output name, the
//!   input names that produced it.
//! - [`Lineage`] — a map, its reverse view and the [`MapVersion`] they were built
//!   under, folded forward from successive [`History`] values.
//! - [`NamePolicy`] and [`DefaultNamePolicy`] — how elements are named, with the
//!   positional `FaceN`/`EdgeN` scheme as the default.
//!
//! This crate has no workspace dependencies. It must not learn about geometry,
//! booleans, documents, selection or persistence formats.

use std::collections::BTreeMap;

// ---------------------------------------------------------------------------
// History and element references
// ---------------------------------------------------------------------------

/// A reference to one sub-element of a shape (a face, an edge, …), by a stable
/// name rather than a positional index.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ElementRef<T> {
    /// The sub-element itself.
    pub shape: T,
    /// Its stable name (e.g. `Face6`, or an encoded lineage token).
    pub name: String,
}

/// Which input of an operation an element came from.
///
/// Operands are numbered by **position**, not identity: [`OperandId::A`] is the
/// first input the caller passed (`a` of `fuse`/`cut`), [`OperandId::B`] the
/// second. A positional label is enough because an operation always has the same
/// operands in the same order; the name is the lineage layer's decision, not the
/// backend's.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct OperandId(u16);

impl OperandId {
    /// The first operand of an operation.
    pub const A: Self = Self(0);
    /// The second operand of an operation.
    pub const B: Self = Self(1);

    /// An operand id from its 0-based position.
    pub const fn new(position: u16) -> Self {
        Self(position)
    }

    /// The 0-based position of the operand.
    pub const fn position(self) -> u16 {
        self.0
    }
}

/// A kernel-independent identity for one sub-element of an operand.
///
/// The ordinal is 1-based and follows the operand's own enumeration for that kind,
/// so `ElementId::new(OperandId::A, "Face", 3)` is the third face of the first
/// operand. `kind` is the caller-supplied display label (`Face`, `Edge`, …); this
/// crate never names a geometry type itself, exactly as [`NamePolicy::name`] takes
/// the prefix from its caller.
///
/// Identity is *what produced* an element, never what it is called. Two coincident
/// operands can each have a `Face1`, and only the operand id tells them apart;
/// naming stays a policy decision.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ElementId {
    operand: OperandId,
    kind: String,
    ordinal: usize,
}

impl ElementId {
    /// The element at `ordinal` (1-based) of `kind` in `operand`.
    pub fn new(operand: OperandId, kind: impl Into<String>, ordinal: usize) -> Self {
        Self {
            operand,
            kind: kind.into(),
            ordinal,
        }
    }

    /// The operand the element belongs to.
    pub fn operand(&self) -> OperandId {
        self.operand
    }

    /// The element-kind label (`Face`, `Edge`, …).
    pub fn kind(&self) -> &str {
        &self.kind
    }

    /// The 1-based ordinal within that operand's elements of this kind.
    pub fn ordinal(&self) -> usize {
        self.ordinal
    }
}

/// The relation an input element took part in.
///
/// The order is the one a history reports: modified, then generated, then
/// deleted, so a list of [`Provenance`] sorts canonically.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Relation {
    /// The input survives as the named output (possibly reshaped).
    Modified,
    /// The operation created the named output from this input.
    Generated,
    /// The input does not survive into the output.
    Deleted,
}

/// One naming derivation, distinct from production provenance.
///
/// A [`History`]'s `modified`/`generated` lists answer "which *input* element
/// produced this output". A derivation answers a different question: which
/// *result* element gave this result element its name. Upstream's element-map
/// builder names unnamed lower elements after the named upper element that
/// contains them (`…;U1`) and unnamed higher elements after their named boundary
/// (`…;L`); each such step is one `Derivation`. The relation is incidence, not
/// provenance, so it carries no operand identity.
///
/// When a lineage folds a derivation into its reverse map, it resolves `source`
/// through the producers already recorded for it, so a chain
/// (`Face1 → Edge2 → Vertex3`) reports the operand elements that named `Face1`
/// rather than an intermediate result element. That keeps every reverse-map
/// producer an operand element name, and matches upstream's encoded names, which
/// carry the source element's whole provenance.
#[derive(Debug, Clone, PartialEq)]
pub struct Derivation<T> {
    /// The element whose name was used.
    pub source: ElementRef<T>,
    /// The element that was named from it.
    pub target: ElementRef<T>,
}

/// The identity behind one relation in a [`History`].
///
/// A history's relation lists carry sub-shape handles and positional names; a
/// `Provenance` says *which operand and which input element* produced a given
/// output name. Entries are self-describing — the relation and output name are
/// repeated — so they never depend on the order of the relation lists, and two
/// operands that both call an element `Face1` stay distinguishable.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Provenance {
    /// The input element's identity.
    pub input: ElementId,
    /// What happened to it.
    pub relation: Relation,
    /// The positional name of the output, for modified/generated relations.
    pub output: Option<String>,
}

impl Provenance {
    /// A provenance entry for `input` under `relation`.
    pub fn new(input: ElementId, relation: Relation, output: Option<String>) -> Self {
        Self {
            input,
            relation,
            output,
        }
    }
}

/// What one operation did to its inputs, in the kernel's own terms.
///
/// This is the raw material for the element map: a mapper folds it into stable names.
/// The kernel reports exactly what it knows; the mapper can be no better than this.
#[derive(Debug, Clone, PartialEq)]
pub struct History<T> {
    /// `(input, output)` pairs the operation created.
    pub generated: Vec<(ElementRef<T>, ElementRef<T>)>,
    /// `(input, output)` pairs the operation changed.
    pub modified: Vec<(ElementRef<T>, ElementRef<T>)>,
    /// Inputs that do not survive into the output.
    pub deleted: Vec<ElementRef<T>>,
    /// `(input, output)` pairs where the input survives unchanged (`IsSame`).
    ///
    /// Upstream names these by copying the input's name; they are not
    /// modifications, so they stay out of `modified`/`generated` and out of the
    /// provenance side table.
    pub identity: Vec<(ElementRef<T>, ElementRef<T>)>,
    /// Naming steps derived from the result's own incidence graph.
    ///
    /// Each entry names `target` after `source`; see [`Derivation`]. They are
    /// folded into the reverse map as naming relations, never as operand
    /// provenance.
    pub derived: Vec<Derivation<T>>,
    /// The identity behind every input relation above.
    ///
    /// One entry per `modified`/`generated` pair and per `deleted` input. Entries
    /// are self-describing, so a backend may sort them for a canonical history;
    /// [`History`] does not infer identity from the positional names. A history
    /// built without provenance (a null backend, a test literal) leaves this empty.
    /// This is *metadata*: it never adds an unchanged element to the history.
    pub provenance: Vec<Provenance>,
}

impl<T> Default for History<T> {
    fn default() -> Self {
        Self {
            generated: Vec::new(),
            modified: Vec::new(),
            deleted: Vec::new(),
            identity: Vec::new(),
            derived: Vec::new(),
            provenance: Vec::new(),
        }
    }
}

impl<T> History<T> {
    /// An operation that reports no lineage (e.g. the null backend).
    pub fn is_empty(&self) -> bool {
        self.generated.is_empty()
            && self.modified.is_empty()
            && self.deleted.is_empty()
            && self.identity.is_empty()
            && self.derived.is_empty()
            && self.provenance.is_empty()
    }

    /// Record a modified relation together with its input identity.
    ///
    /// Using the `push_*` helpers keeps the relation lists and the provenance
    /// side table in step; a caller that records neither can keep pushing onto the
    /// public fields.
    pub fn push_modified(&mut self, input: ElementRef<T>, input_id: ElementId, output: ElementRef<T>) {
        self.provenance.push(Provenance::new(
            input_id,
            Relation::Modified,
            Some(output.name.clone()),
        ));
        self.modified.push((input, output));
    }

    /// Record a generated relation together with its input identity.
    pub fn push_generated(
        &mut self,
        input: ElementRef<T>,
        input_id: ElementId,
        output: ElementRef<T>,
    ) {
        self.provenance.push(Provenance::new(
            input_id,
            Relation::Generated,
            Some(output.name.clone()),
        ));
        self.generated.push((input, output));
    }

    /// Record a deleted input together with its identity.
    pub fn push_deleted(&mut self, input: ElementRef<T>, input_id: ElementId) {
        self.provenance
            .push(Provenance::new(input_id, Relation::Deleted, None));
        self.deleted.push(input);
    }

    /// Record an input that survives unchanged as `output`.
    ///
    /// This is the identity (`IsSame`) mapping upstream's `mapSubElement` copies:
    /// the input's name comes to mean the same sub-shape in the result. It is not
    /// a modification, so it adds no provenance entry.
    pub fn push_identity(&mut self, input: ElementRef<T>, output: ElementRef<T>) {
        self.identity.push((input, output));
    }

    /// Record one naming step derived from the result's incidence graph.
    pub fn push_derived(&mut self, source: ElementRef<T>, target: ElementRef<T>) {
        self.derived.push(Derivation { source, target });
    }

    /// The provenance entries for one relation kind, in reported order.
    pub fn provenance_for(&self, relation: Relation) -> impl Iterator<Item = &Provenance> {
        self.provenance
            .iter()
            .filter(move |entry| entry.relation == relation)
    }
}

/// The stable-name registry carried with a shape: `name -> current sub-shape`.
///
/// It is a cache and a persistence format, not the identity itself; names should be
/// re-derivable from [`History`] where possible, which is what lets old documents be
/// migrated after a rebuild.
#[derive(Debug, Clone, PartialEq)]
pub struct ElementMap<T> {
    entries: BTreeMap<String, T>,
}

impl<T> Default for ElementMap<T> {
    fn default() -> Self {
        Self {
            entries: BTreeMap::new(),
        }
    }
}

impl<T> ElementMap<T> {
    pub fn new() -> Self {
        Self::default()
    }

    /// Record the current sub-shape for `name`.
    pub fn insert(&mut self, name: impl Into<String>, shape: T) {
        self.entries.insert(name.into(), shape);
    }

    /// The current sub-shape for `name`, if the name still resolves.
    pub fn get(&self, name: &str) -> Option<&T> {
        self.entries.get(name)
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Iterate `(name, shape)` in name order.
    pub fn iter(&self) -> impl Iterator<Item = (&str, &T)> {
        self.entries.iter().map(|(k, v)| (k.as_str(), v))
    }

    /// Fold an operation's [`History`] into the map.
    ///
    /// Each `generated`/`modified` input name comes to mean its output sub-shape, so a
    /// reference recorded before the operation still resolves after the rebuild. When
    /// one input maps to several outputs, the **first** output wins: a single name
    /// cannot denote a set, and composing names (`Face6a`, `Face6b`) is the mapper's
    /// job (see `docs/project/research/geometry-and-topology.md` §9). An input the operation deleted is
    /// removed, so a stale reference fails to resolve instead of pointing at geometry
    /// that is no longer there.
    ///
    /// Input names are the names the history was built with. With positional names
    /// that is the input shape's own ordinal; a lineage-derived naming scheme can feed
    /// the same method with better names later.
    ///
    /// Deletions retire the *previous* binding first: the deleted list describes
    /// inputs, whose positional spelling may coincide with an output of this same
    /// operation (operands and results have independent positional namespaces).
    /// Retiring first lets this operation's own outputs keep their names while a
    /// stale binding still fails to resolve.
    pub fn apply(&mut self, history: &History<T>)
    where
        T: Clone,
    {
        for deleted in &history.deleted {
            self.entries.remove(&deleted.name);
        }
        // Identity first: an unchanged sub-shape keeps its input name, exactly as
        // upstream's `mapSubElement` runs before the modified/generated pass.
        for (input, output) in &history.identity {
            self.entries
                .entry(input.name.clone())
                .or_insert_with(|| output.shape.clone());
        }
        for (input, output) in history.modified.iter().chain(&history.generated) {
            self.entries
                .entry(input.name.clone())
                .or_insert_with(|| output.shape.clone());
        }
    }

    /// A map seeded from one operation's history.
    pub fn from_history(history: &History<T>) -> Self
    where
        T: Clone,
    {
        let mut map = Self::new();
        map.apply(history);
        map
    }
}

// ---------------------------------------------------------------------------
// Naming policy
// ---------------------------------------------------------------------------

/// How sub-elements are turned into stable names.
///
/// The policy is the only place a naming *scheme* lives: the map, the reverse map
/// and the lineage record what happened, never what an element ought to be called.
/// Implementations must be pure — same arguments, same name — with no geometry, no
/// clock, no document and no counter.
///
/// `kind` is a display prefix supplied by the caller (`Face`, `Edge`, …); this crate
/// cannot name a geometry type itself. `ordinal` is 1-based, matching FreeCAD's
/// element naming (`Face1` is the first face), so a positional policy reproduces the
/// names the backends already emit.
pub trait NamePolicy {
    /// The name of the element at `ordinal` (1-based) of `kind`.
    ///
    /// `input` is the lineage name of the input element that produced this one, when
    /// the operation reported one. A positional policy ignores it; a
    /// lineage-derived policy composes it.
    fn name(&self, kind: &str, ordinal: usize, input: Option<&str>) -> String;

    /// The names of `count` outputs produced by the single input named `stem`.
    ///
    /// This is where a one-input-to-many-outputs relation stops being a set with no
    /// spelling. The returned names are in output order and must be distinct.
    fn split(&self, stem: &str, count: usize) -> Vec<String>;
}

/// Today's naming: positional `{kind}{ordinal}`, and `stem` plus a letter for a
/// split.
///
/// It reproduces exactly the names the geometry backends already produce (`Face1`
/// is the first face, `Edge12` the twelfth edge), so adopting the lineage layer
/// changes no visible name until a different policy is supplied.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct DefaultNamePolicy;

impl DefaultNamePolicy {
    /// The identifier recorded in [`MapVersion::policy`] for this policy.
    pub const ID: &'static str = "positional";
}

impl NamePolicy for DefaultNamePolicy {
    fn name(&self, kind: &str, ordinal: usize, input: Option<&str>) -> String {
        // Positional naming ignores provenance: `Face6` is the sixth face, not a
        // reference to whatever produced it.
        let _ = input;
        format!("{kind}{ordinal}")
    }

    fn split(&self, stem: &str, count: usize) -> Vec<String> {
        match count {
            // No outputs, no names.
            0 => Vec::new(),
            // One input, one output: the input keeps its own name, exactly as
            // today's one-to-one folding does.
            1 => vec![stem.to_string()],
            // Two or more: compose a suffix in output order, `a`, `b`, … `z`,
            // `aa`, `ab`, … so one stem cannot silently denote several elements.
            _ => (0..count)
                .map(|index| format!("{stem}{}", suffix(index)))
                .collect(),
        }
    }
}

/// A lowercase `a`-based suffix: `0 -> "a"`, `25 -> "z"`, `26 -> "aa"`, `27 ->
/// `"ab"` (spreadsheet columns).
fn suffix(mut index: usize) -> String {
    let mut letters = Vec::new();
    loop {
        letters.push(b'a' + (index % 26) as u8);
        if index < 26 {
            break;
        }
        index = index / 26 - 1;
    }
    letters.reverse();
    String::from_utf8(letters).expect("letters are ASCII")
}

// ---------------------------------------------------------------------------
// Map version
// ---------------------------------------------------------------------------

/// The naming scheme a [`Lineage`] was folded under.
///
/// Three independent axes, kept comparable on purpose: a stored map is only valid
/// under the rules that produced it, so a version mismatch must invalidate rather
/// than guess. Nothing is persisted yet — this is not `ElementMapVersion`, and the
/// default describes today's in-memory behaviour only.
///
/// - `encoding` — how names are encoded; [`MapVersion::ENCODING_PLAIN`] means plain
///   positional names with no cross-operation tokens.
/// - `policy` — the [`NamePolicy`]'s identifier (see [`DefaultNamePolicy::ID`]).
/// - `kernel` — the kernel whose history the map was folded from, if recorded.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct MapVersion {
    encoding: u16,
    policy: String,
    kernel: Option<String>,
}

impl MapVersion {
    /// Plain names: no encoded lineage tokens, only what a positional policy emits.
    pub const ENCODING_PLAIN: u16 = 0;

    /// Upstream's `TopoShape::getElementMapVersion` opcode (`OpCodes::Version`),
    /// which is 15 at the pinned upstream commit (`33adf61`).
    pub const OPCODE_VERSION: u16 = 15;

    /// The OCCT version upstream reports, stabilized to 7.2.0 (`0x070200`) so the
    /// string does not churn with the linked kernel.
    pub const OCCT_VERSION: u32 = 0x070200;

    pub fn new(encoding: u16, policy: impl Into<String>, kernel: Option<String>) -> Self {
        Self {
            encoding,
            policy: policy.into(),
            kernel,
        }
    }

    /// The composite `ElementMapVersion` string this version describes.
    ///
    /// Upstream composes `"<opcode>.<occt-version-hex>."` with the naming
    /// policy's own version tag. The three version axes are appended here so a
    /// stored map is only valid under the rules that produced it; the leading
    /// fields match upstream's spelling so callers that only compare the prefix
    /// (or, as the Part tests do, test for non-emptiness) see a faithful value.
    pub fn element_map_version(&self) -> String {
        format!(
            "{}.{:x}.{}.{}",
            Self::OPCODE_VERSION, Self::OCCT_VERSION, self.encoding, self.policy
        )
    }

    /// The encoding revision.
    pub fn encoding(&self) -> u16 {
        self.encoding
    }

    /// The naming policy identifier (see [`DefaultNamePolicy::ID`]).
    pub fn policy(&self) -> &str {
        &self.policy
    }

    /// The kernel whose history produced the map, when recorded.
    pub fn kernel(&self) -> Option<&str> {
        self.kernel.as_deref()
    }
}

impl Default for MapVersion {
    /// Today's behaviour: plain positional names under [`DefaultNamePolicy`], with
    /// no kernel recorded (names are kernel-independent so far).
    fn default() -> Self {
        Self {
            encoding: Self::ENCODING_PLAIN,
            policy: DefaultNamePolicy::ID.to_string(),
            kernel: None,
        }
    }
}

// ---------------------------------------------------------------------------
// Reverse map
// ---------------------------------------------------------------------------

/// The element map read backwards: for each **output** name, the **input** names
/// that produced it.
///
/// [`ElementMap`] answers "what does this name mean now"; `ReverseMap` answers
/// "what produced this name". It cannot be derived from an [`ElementMap`], whose
/// values are opaque payloads carrying no name, so it is folded from [`History`]
/// alongside the forward map, by [`Lineage`].
///
/// A derived name (`Face1` naming its edges) is recorded against the producers
/// that named its source, so the producer lists always contain operand element
/// names.
///
/// Representation: `output name -> input names`, both `String`. Keys live in a
/// `BTreeMap`, so iteration is in name order; each producer list is de-duplicated
/// and **sorted**, so the reverse map is canonical for a set of relations — the
/// order in which a history reported them does not change the result. Every output
/// the history reported is kept, including the extra outputs the forward map's
/// first-output-wins rule cannot represent.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ReverseMap {
    entries: BTreeMap<String, Vec<String>>,
}

impl ReverseMap {
    pub fn new() -> Self {
        Self::default()
    }

    /// The input names that produced `output`, in name order; empty if unknown.
    pub fn inputs_for(&self, output: &str) -> &[String] {
        self.entries.get(output).map(Vec::as_slice).unwrap_or(&[])
    }

    /// The input names that produced `output`, if `output` is a known name.
    pub fn get(&self, output: &str) -> Option<&[String]> {
        self.entries.get(output).map(Vec::as_slice)
    }

    /// Whether `output` is a known name.
    pub fn contains(&self, output: &str) -> bool {
        self.entries.contains_key(output)
    }

    /// The number of output names with at least one recorded producer.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Iterate `(output, inputs)` in output-name order.
    pub fn iter(&self) -> impl Iterator<Item = (&str, &[String])> {
        self.entries
            .iter()
            .map(|(output, inputs)| (output.as_str(), inputs.as_slice()))
    }

    /// Record that `input` produced `output`. Re-recording a relation is a no-op.
    fn record(&mut self, input: &str, output: &str) {
        let inputs = self.entries.entry(output.to_string()).or_default();
        if let Err(index) = inputs.binary_search_by(|name| name.as_str().cmp(input)) {
            inputs.insert(index, input.to_string());
        }
    }

    /// Retire a deleted name: it is no longer an element, so it is dropped both as
    /// an output key and as a producer of other names.
    fn retire(&mut self, name: &str) {
        self.entries.remove(name);
        self.entries.retain(|_, inputs| {
            inputs.retain(|input| input != name);
            !inputs.is_empty()
        });
    }
}

// ---------------------------------------------------------------------------
// Lineage
// ---------------------------------------------------------------------------

/// A shape's accumulated provenance: the forward map, its reverse view, and the
/// version of the rules that produced them.
///
/// One [`History`] is one operation's edge set; a `Lineage` is the per-shape
/// accumulation, folded forward by [`apply`](Lineage::apply). It records only what
/// histories report; it never queries geometry, and it never invents a name — that
/// is the [`NamePolicy`]'s job.
#[derive(Debug, Clone, PartialEq)]
pub struct Lineage<T> {
    map: ElementMap<T>,
    reverse: ReverseMap,
    version: MapVersion,
}

impl<T> Default for Lineage<T> {
    /// An empty lineage under [`MapVersion::default`].
    fn default() -> Self {
        Self::new(MapVersion::default())
    }
}

impl<T> Lineage<T> {
    /// An empty lineage whose rules are described by `version`.
    pub fn new(version: MapVersion) -> Self {
        Self {
            map: ElementMap::new(),
            reverse: ReverseMap::new(),
            version,
        }
    }

    /// A lineage seeded from one operation's history.
    pub fn from_history(history: &History<T>, version: MapVersion) -> Self
    where
        T: Clone,
    {
        let mut lineage = Self::new(version);
        lineage.apply(history);
        lineage
    }

    /// The forward map: a name recorded before an operation still resolves after it.
    pub fn map(&self) -> &ElementMap<T> {
        &self.map
    }

    /// The reverse view: which input names produced each output name.
    pub fn reverse(&self) -> &ReverseMap {
        &self.reverse
    }

    /// The rules this lineage was folded under.
    pub fn version(&self) -> &MapVersion {
        &self.version
    }

    /// The current sub-shape for `name`, if the name still resolves.
    pub fn get(&self, name: &str) -> Option<&T> {
        self.map.get(name)
    }

    /// The input names that produced `output`, in name order; empty if unknown.
    pub fn inputs_for(&self, output: &str) -> &[String] {
        self.reverse.inputs_for(output)
    }

    /// Fold one operation's [`History`] into the lineage.
    ///
    /// The forward map is exactly [`ElementMap::apply`]: an input name comes to mean
    /// its output sub-shape, the first output wins when one input produced several,
    /// and a deleted input's name is retired. Deletions are retired *before* this
    /// operation's own relations are recorded, because inputs and outputs have
    /// independent positional namespaces. The reverse map records every
    /// `(input, output)` relation (including the outputs the forward map cannot keep),
    /// the identity names, and the derived (incidence) names.
    ///
    /// Folding is **idempotent**: applying the same history again leaves both maps
    /// unchanged. The forward map's `or_insert` cannot rebind an existing name and
    /// the reverse map de-duplicates its producers, so re-running an operation's
    /// history is a no-op. It is not commutative across *different* histories: like
    /// the forward map, the fold is order-dependent, and the first binding wins.
    pub fn apply(&mut self, history: &History<T>)
    where
        T: Clone,
    {
        // Retire the previous bindings a deleted input named, then record this
        // operation's own naming relations. Inputs and outputs have independent
        // positional namespaces, so a deleted `Face1` must not erase the result's
        // own `Face1`.
        for deleted in &history.deleted {
            self.reverse.retire(&deleted.name);
        }
        // Same order as `ElementMap::apply`, so the two maps agree on "first".
        for (input, output) in &history.identity {
            self.reverse.record(&input.name, &output.name);
        }
        for (input, output) in history.modified.iter().chain(&history.generated) {
            self.reverse.record(&input.name, &output.name);
        }
        // Derived names are named *from* a result element, not produced by an
        // operand. Resolve the source to the producers that named it, so every
        // reverse-map producer stays an operand element name (which is also a
        // forward-map key) rather than an intermediate derived name. A source
        // with no recorded producer is reported as itself.
        for derivation in &history.derived {
            let producers = self
                .reverse
                .inputs_for(&derivation.source.name)
                .to_vec();
            if producers.is_empty() {
                self.reverse
                    .record(&derivation.source.name, &derivation.target.name);
            } else {
                for producer in producers {
                    self.reverse.record(&producer, &derivation.target.name);
                }
            }
        }
        self.map.apply(history);
    }
}

#[cfg(test)]
mod tests {
    use super::{
        DefaultNamePolicy, ElementId, ElementMap, ElementRef, History, Lineage, MapVersion,
        NamePolicy, OperandId, Provenance, Relation, ReverseMap,
    };

    fn element(shape: &str, name: &str) -> ElementRef<String> {
        ElementRef {
            shape: shape.to_string(),
            name: name.to_string(),
        }
    }

    fn producers<'a>(reverse: &'a ReverseMap, output: &str) -> Vec<&'a str> {
        reverse
            .inputs_for(output)
            .iter()
            .map(String::as_str)
            .collect()
    }

    fn map_entries(map: &ElementMap<String>) -> Vec<(&str, &String)> {
        map.iter().collect()
    }

    fn reverse_entries(reverse: &ReverseMap) -> Vec<(&str, &[String])> {
        reverse.iter().collect()
    }

    #[test]
    fn map_inserts_gets_and_counts() {
        let mut map = ElementMap::new();
        assert!(map.is_empty());
        assert_eq!(map.len(), 0);

        map.insert("Face1", "face-1".to_string());
        map.insert("Face2", "face-2".to_string());

        assert_eq!(map.len(), 2);
        assert!(!map.is_empty());
        assert_eq!(map.get("Face1"), Some(&"face-1".to_string()));
        assert_eq!(map.get("Face2"), Some(&"face-2".to_string()));
        assert_eq!(map.get("Face3"), None);
    }

    #[test]
    fn map_iterates_in_name_order() {
        let mut map = ElementMap::new();
        map.insert("Face10", "ten".to_string());
        map.insert("Face2", "two".to_string());
        map.insert("Edge1", "edge".to_string());

        let names: Vec<&str> = map.iter().map(|(name, _)| name).collect();
        assert_eq!(names, ["Edge1", "Face10", "Face2"]);

        let values: Vec<&str> = map.iter().map(|(_, shape)| shape.as_str()).collect();
        assert_eq!(values, ["edge", "ten", "two"]);
    }

    #[test]
    fn apply_keeps_the_first_output_for_a_name() {
        let input = element("in", "Face1");
        let first = element("out-1", "Face2");
        let second = element("out-2", "Face3");

        let mut history = History::default();
        // Two outputs for one input: only the first output is recorded.
        history.generated.push((input.clone(), first.clone()));
        history.generated.push((input, second));

        let mut map = ElementMap::new();
        map.apply(&history);
        assert_eq!(map.len(), 1);
        assert_eq!(map.get("Face1"), Some(&"out-1".to_string()));
    }

    #[test]
    fn apply_does_not_rebind_an_existing_name() {
        let mut map = ElementMap::new();
        map.insert("Face1", "pinned".to_string());

        let mut history = History::default();
        history
            .modified
            .push((element("in", "Face1"), element("out", "Face2")));
        map.apply(&history);

        assert_eq!(map.get("Face1"), Some(&"pinned".to_string()));
    }

    #[test]
    fn apply_removes_deleted_names() {
        let mut map = ElementMap::new();
        map.insert("Face1", "kept".to_string());
        map.insert("Face2", "gone".to_string());

        let history = History {
            deleted: vec![element("stale", "Face2")],
            ..History::default()
        };
        map.apply(&history);

        assert_eq!(map.len(), 1);
        assert_eq!(map.get("Face1"), Some(&"kept".to_string()));
        assert_eq!(map.get("Face2"), None);
    }

    #[test]
    fn from_history_seeds_all_relations() {
        let mut history = History::default();
        history
            .modified
            .push((element("in-m", "Face1"), element("out-m", "Face2")));
        history
            .generated
            .push((element("in-g", "Edge1"), element("out-g", "Edge2")));
        history.deleted.push(element("gone", "Face9"));

        let map = ElementMap::from_history(&history);

        assert_eq!(map.len(), 2);
        assert_eq!(map.get("Face1"), Some(&"out-m".to_string()));
        assert_eq!(map.get("Edge1"), Some(&"out-g".to_string()));
        assert_eq!(map.get("Face9"), None);
    }

    #[test]
    fn empty_history_is_empty() {
        assert!(History::<String>::default().is_empty());

        let mut history = History::default();
        history.deleted.push(element("gone", "Face1"));
        assert!(!history.is_empty());
    }

    // -- reverse map --------------------------------------------------------

    #[test]
    fn reverse_map_records_every_producer_sorted_and_deduplicated() {
        let mut history = History::default();
        history
            .generated
            .push((element("in-1", "Face3"), element("out-1", "Face2")));
        history
            .generated
            .push((element("in-2", "Face1"), element("out-2", "Face2")));
        // Report the same relation twice: a name is a set of producers, not a count.
        history
            .generated
            .push((element("in-1", "Face3"), element("out-3", "Face2")));

        let lineage = Lineage::from_history(&history, MapVersion::default());

        assert_eq!(producers(lineage.reverse(), "Face2"), ["Face1", "Face3"]);
        assert_eq!(lineage.reverse().len(), 1);
        assert!(lineage.reverse().contains("Face2"));
        assert_eq!(producers(lineage.reverse(), "Face9"), Vec::<&str>::new());
    }

    #[test]
    fn reverse_map_keeps_the_outputs_the_forward_map_cannot() {
        let input = element("in", "Face1");
        let mut history = History::default();
        history
            .generated
            .push((input.clone(), element("out-1", "Face2")));
        history.generated.push((input, element("out-2", "Face3")));

        let lineage = Lineage::from_history(&history, MapVersion::default());

        // The forward map can only keep one output per name (first wins) …
        assert_eq!(lineage.map().len(), 1);
        assert_eq!(lineage.get("Face1"), Some(&"out-1".to_string()));
        // … but the reverse map is the full relation: both outputs were produced.
        assert_eq!(producers(lineage.reverse(), "Face2"), ["Face1"]);
        assert_eq!(producers(lineage.reverse(), "Face3"), ["Face1"]);
    }

    #[test]
    fn reverse_map_iterates_in_output_name_order() {
        let mut history = History::default();
        history
            .generated
            .push((element("a", "A"), element("b", "Face10")));
        history
            .generated
            .push((element("c", "C"), element("d", "Face2")));
        history
            .generated
            .push((element("e", "E"), element("f", "Edge1")));

        let lineage = Lineage::from_history(&history, MapVersion::default());
        let outputs: Vec<&str> = lineage
            .reverse()
            .iter()
            .map(|(output, _)| output)
            .collect();

        assert_eq!(outputs, ["Edge1", "Face10", "Face2"]);
    }

    #[test]
    fn reverse_map_retires_deleted_outputs() {
        let mut lineage = Lineage::from_history(
            &History {
                modified: vec![(element("in", "Face1"), element("out", "Face2"))],
                ..History::default()
            },
            MapVersion::default(),
        );
        assert_eq!(producers(lineage.reverse(), "Face2"), ["Face1"]);

        lineage.apply(&History {
            deleted: vec![element("stale", "Face2")],
            ..History::default()
        });

        assert!(!lineage.reverse().contains("Face2"));
        assert!(lineage.reverse().inputs_for("Face2").is_empty());
        // The forward map retired the name as well; the pinned input survives.
        assert_eq!(lineage.get("Face2"), None);
        assert_eq!(lineage.get("Face1"), Some(&"out".to_string()));
    }

    #[test]
    fn reverse_map_retires_a_deleted_producer() {
        let mut lineage = Lineage::from_history(
            &History {
                generated: vec![(element("in", "Face1"), element("out", "Face10"))],
                ..History::default()
            },
            MapVersion::default(),
        );
        assert_eq!(producers(lineage.reverse(), "Face10"), ["Face1"]);

        lineage.apply(&History {
            deleted: vec![element("in", "Face1")],
            ..History::default()
        });

        // Face1 no longer names an element, so it is not reported as a producer.
        assert!(!lineage.reverse().contains("Face10"));
        assert!(lineage.reverse().inputs_for("Face10").is_empty());
    }

    // -- lineage ------------------------------------------------------------

    #[test]
    fn lineage_folds_successive_histories_forward_and_backward() {
        let first = History {
            generated: vec![(element("in", "Face1"), element("out", "Face2"))],
            ..History::default()
        };
        let second = History {
            modified: vec![(element("in", "Face2"), element("out", "Face3"))],
            ..History::default()
        };

        let mut lineage = Lineage::from_history(&first, MapVersion::default());
        lineage.apply(&second);

        // One logical element, two names that still resolve to its latest shape.
        assert_eq!(lineage.get("Face1"), Some(&"out".to_string()));
        assert_eq!(lineage.get("Face2"), Some(&"out".to_string()));
        assert_eq!(lineage.map().len(), 2);

        // The reverse view records each step's edge.
        assert_eq!(producers(lineage.reverse(), "Face2"), ["Face1"]);
        assert_eq!(producers(lineage.reverse(), "Face3"), ["Face2"]);
    }

    #[test]
    fn lineage_forward_map_matches_element_map_folding() {
        let mut history = History::default();
        history
            .modified
            .push((element("in-m", "Face1"), element("out-m", "Face2")));
        history
            .generated
            .push((element("in-g", "Edge1"), element("out-g", "Edge2")));
        history.deleted.push(element("gone", "Face9"));

        let lineage = Lineage::from_history(&history, MapVersion::default());

        // The lineage adds a reverse view; the forward map is untouched behaviour.
        assert_eq!(lineage.map(), &ElementMap::from_history(&history));
    }

    #[test]
    fn applying_the_same_history_twice_changes_nothing() {
        let mut history = History::default();
        history
            .modified
            .push((element("in-m", "Face1"), element("out-m", "Face2")));
        history
            .generated
            .push((element("in-g", "Edge1"), element("out-g", "Edge2")));
        history
            .generated
            .push((element("in-g2", "Edge1"), element("out-g2", "Edge3")));
        history.deleted.push(element("gone", "Face9"));

        let mut once = Lineage::new(MapVersion::default());
        once.apply(&history);

        let mut twice = Lineage::new(MapVersion::default());
        twice.apply(&history);
        twice.apply(&history);

        // Equal values, and the same iteration order (the maps are ordered).
        assert_eq!(once, twice);
        assert_eq!(map_entries(once.map()), map_entries(twice.map()));
        assert_eq!(
            reverse_entries(once.reverse()),
            reverse_entries(twice.reverse())
        );

        // A rebuild from the same history is identical to a continuation.
        let mut rebuilt = Lineage::new(MapVersion::default());
        rebuilt.apply(&history);
        assert_eq!(once, rebuilt);
    }

    #[test]
    fn lineage_exposes_map_reverse_and_version() {
        let version = MapVersion::new(7, "positional", Some("occt".to_string()));
        let lineage = Lineage::<String>::new(version.clone());

        assert!(lineage.map().is_empty());
        assert!(lineage.reverse().is_empty());
        assert_eq!(lineage.version(), &version);
        assert_eq!(lineage.get("Face1"), None);
        assert!(lineage.inputs_for("Face1").is_empty());
        assert_eq!(Lineage::<String>::default().version(), &MapVersion::default());
    }

    // -- naming policy and version ------------------------------------------

    #[test]
    fn default_name_policy_reproduces_positional_names() {
        let policy = DefaultNamePolicy;

        assert_eq!(policy.name("Face", 1, None), "Face1");
        assert_eq!(policy.name("Face", 6, None), "Face6");
        assert_eq!(policy.name("Edge", 12, Some("Face3")), "Edge12");
        assert_eq!(policy.name("Vertex", 100, None), "Vertex100");
        assert_eq!(policy.name("Solid", 1, None), "Solid1");
        assert_eq!(policy.name("Shell", 2, None), "Shell2");

        let dynamic: &dyn NamePolicy = &DefaultNamePolicy;
        assert_eq!(dynamic.name("Edge", 1, None), "Edge1");
    }

    #[test]
    fn default_name_policy_splits_one_input_into_distinct_names() {
        let policy = DefaultNamePolicy;

        assert!(policy.split("Face6", 0).is_empty());
        assert_eq!(policy.split("Face6", 1), vec!["Face6".to_string()]);
        assert_eq!(
            policy.split("Face6", 3),
            vec![
                "Face6a".to_string(),
                "Face6b".to_string(),
                "Face6c".to_string()
            ]
        );

        let many = policy.split("Face6", 28);
        assert_eq!(many.len(), 28);
        assert_eq!(many[25], "Face6z");
        assert_eq!(many[26], "Face6aa");
        assert_eq!(many[27], "Face6ab");

        let mut unique = many.clone();
        unique.sort();
        unique.dedup();
        assert_eq!(unique.len(), many.len());
    }

    #[test]
    fn map_version_defaults_to_plain_positional_names_and_compares() {
        let default = MapVersion::default();

        assert_eq!(default.encoding(), MapVersion::ENCODING_PLAIN);
        assert_eq!(default.policy(), DefaultNamePolicy::ID);
        assert_eq!(default.kernel(), None);

        assert_eq!(default, MapVersion::new(0, "positional", None));
        assert_ne!(default, MapVersion::new(1, "positional", None));
        assert_ne!(default, MapVersion::new(0, "lineage", None));
        assert_ne!(
            default,
            MapVersion::new(0, "positional", Some("occt".to_string()))
        );
        assert!(MapVersion::new(0, "positional", None) < MapVersion::new(1, "positional", None));

        let pinned = MapVersion::new(7, "lineage", Some("occt-7.2".to_string()));
        assert_eq!(pinned.encoding(), 7);
        assert_eq!(pinned.policy(), "lineage");
        assert_eq!(pinned.kernel(), Some("occt-7.2"));
    }

    #[test]
    fn the_element_map_version_is_a_non_empty_composite_that_tracks_the_rules() {
        let default = MapVersion::default();
        // Upstream's spelling: `OpCodes::Version` (15 at `33adf61`), the stabilized
        // OCCT version in hex (7.2.0 → 70200), then this kernel's encoding and
        // policy tags.
        assert_eq!(default.element_map_version(), "15.70200.0.positional");
        assert!(!default.element_map_version().is_empty());

        // A different encoding or policy is a different map version, so a stored
        // map cannot be read under rules that did not produce it.
        assert_ne!(
            MapVersion::new(1, "positional", None).element_map_version(),
            default.element_map_version()
        );
        assert_ne!(
            MapVersion::new(0, "lineage", None).element_map_version(),
            default.element_map_version()
        );
    }

    // -- provenance identity -------------------------------------------------

    #[test]
    fn element_ids_distinguish_two_operands_with_the_same_positional_name() {
        let a = ElementId::new(OperandId::A, "Face", 1);
        let b = ElementId::new(OperandId::B, "Face", 1);

        assert_ne!(a, b, "the same ordinal in different operands is not one element");
        assert_eq!(a.operand(), OperandId::A);
        assert_eq!(a.kind(), "Face");
        assert_eq!(a.ordinal(), 1);
        assert_eq!(b.operand().position(), 1);
        assert_eq!(OperandId::new(0), OperandId::A);
        assert_eq!(OperandId::new(1), OperandId::B);

        // Identity is ordered by operand, then kind, then ordinal.
        assert!(OperandId::A < OperandId::B);
        assert!(ElementId::new(OperandId::A, "Edge", 12) < ElementId::new(OperandId::A, "Face", 1));
        assert!(ElementId::new(OperandId::A, "Face", 1) < ElementId::new(OperandId::A, "Face", 2));
    }

    #[test]
    fn push_helpers_keep_relations_and_provenance_in_step() {
        let mut history = History::default();
        let input_id = ElementId::new(OperandId::B, "Edge", 2);
        let deleted_id = ElementId::new(OperandId::A, "Vertex", 7);

        history.push_modified(element("in", "Edge2"), input_id.clone(), element("out", "Face3"));
        history.push_generated(
            element("in2", "Edge2"),
            input_id.clone(),
            element("out2", "Face4"),
        );
        history.push_deleted(element("gone", "Vertex7"), deleted_id);

        assert_eq!(history.modified.len(), 1);
        assert_eq!(history.generated.len(), 1);
        assert_eq!(history.deleted.len(), 1);
        assert_eq!(history.provenance.len(), 3);
        assert_eq!(
            history.provenance_for(Relation::Modified).collect::<Vec<_>>(),
            vec![&Provenance::new(
                input_id.clone(),
                Relation::Modified,
                Some("Face3".to_string())
            )]
        );
        assert_eq!(history.provenance_for(Relation::Deleted).count(), 1);
        assert_eq!(
            history.provenance[1],
            Provenance::new(input_id, Relation::Generated, Some("Face4".to_string())),
            "the generated entry carries its own identity and output name"
        );
    }

    #[test]
    fn provenance_does_not_change_how_a_history_folds_into_a_map() {
        let mut with_metadata = History::default();
        let mut without_metadata = History::default();
        for (history, annotated) in [
            (&mut with_metadata, true),
            (&mut without_metadata, false),
        ] {
            if annotated {
                history.push_modified(
                    element("in", "Face1"),
                    ElementId::new(OperandId::A, "Face", 1),
                    element("out", "Face2"),
                );
            } else {
                history
                    .modified
                    .push((element("in", "Face1"), element("out", "Face2")));
            }
        }

        let annotated = ElementMap::from_history(&with_metadata);
        let plain = ElementMap::from_history(&without_metadata);
        assert_eq!(map_entries(&annotated), map_entries(&plain));
        assert_eq!(with_metadata.provenance.len(), 1);
        assert!(without_metadata.provenance.is_empty());
    }

    // -- identity and derivation --------------------------------------------

    #[test]
    fn identity_names_fold_into_both_maps_without_provenance() {
        let mut history = History::default();
        history.push_identity(element("in", "Face1"), element("out", "Face1"));

        let lineage = Lineage::from_history(&history, MapVersion::default());
        assert_eq!(lineage.get("Face1"), Some(&"out".to_string()));
        assert_eq!(producers(lineage.reverse(), "Face1"), ["Face1"]);
        // An unchanged element is not a modification and carries no identity
        // entry in the operand provenance table.
        assert!(history.provenance.is_empty());
        assert!(history.modified.is_empty() && history.generated.is_empty());
    }

    #[test]
    fn derived_names_fold_into_the_reverse_map_only() {
        let mut history = History::default();
        history.push_derived(element("face", "Face1"), element("edge", "Edge2"));

        let lineage = Lineage::from_history(&history, MapVersion::default());
        assert_eq!(producers(lineage.reverse(), "Edge2"), ["Face1"]);
        // A derivation is incidence, not production: it binds no forward shape.
        assert!(lineage.map().is_empty());
        assert!(history.provenance.is_empty());
    }

    #[test]
    fn a_deleted_input_does_not_erase_this_historys_own_output_name() {
        // Inputs and outputs have independent positional namespaces, so a tool
        // element `Face1` being deleted must not retire the *result's* `Face1`.
        let mut history = History::default();
        history.deleted.push(element("tool", "Face1"));
        history
            .modified
            .push((element("base", "Face1"), element("out", "Face1")));

        let lineage = Lineage::from_history(&history, MapVersion::default());
        assert_eq!(producers(lineage.reverse(), "Face1"), ["Face1"]);
        assert_eq!(lineage.get("Face1"), Some(&"out".to_string()));
    }

    #[test]
    fn folding_identity_and_derived_relations_is_idempotent() {
        let mut history = History::default();
        history.deleted.push(element("tool", "Face9"));
        history.push_identity(element("in", "Face1"), element("out", "Face1"));
        history.push_derived(element("face", "Face1"), element("edge", "Edge2"));

        let mut once = Lineage::from_history(&history, MapVersion::default());
        let before = (
            map_entries(once.map())
                .into_iter()
                .map(|(name, value)| (name.to_string(), value.clone()))
                .collect::<Vec<_>>(),
            reverse_entries(once.reverse())
                .into_iter()
                .map(|(name, inputs)| (name.to_string(), inputs.to_vec()))
                .collect::<Vec<_>>(),
        );
        once.apply(&history);
        let after = (
            map_entries(once.map())
                .into_iter()
                .map(|(name, value)| (name.to_string(), value.clone()))
                .collect::<Vec<_>>(),
            reverse_entries(once.reverse())
                .into_iter()
                .map(|(name, inputs)| (name.to_string(), inputs.to_vec()))
                .collect::<Vec<_>>(),
        );
        assert_eq!(before, after);
    }
}
