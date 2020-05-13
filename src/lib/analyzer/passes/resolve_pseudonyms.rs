use crate::{
  common::{ Identifier, },
  source::{ SourceRegion, },
  ctx::{ ContextKey, ContextItem, TypeData, },
  ast::{ TypeExpression, TypeExpressionData, Path, },
};

use super::{
  Analyzer,
  support_structures::{ Pseudonym, PseudonymKind, PseudonymPayload, },
  ty_helpers::{ ty_from_anon_data, },
};



/// Iterates the vec of Pseudonymes created by the previous pass and attempts to resolve paths
pub fn resolve_pseudonyms (analyzer: &mut Analyzer, pseudonyms: &mut Vec<Pseudonym>) {
  while let Some(pseudonym) = pseudonyms.pop() {
    resolve_pseudonym(analyzer, pseudonyms, pseudonym);
  }
}


fn try_get_pseudonym (pseudonyms: &mut Vec<Pseudonym>, in_namespace: ContextKey, kind: PseudonymKind, identifier: &Identifier) -> Option<Pseudonym> {
  for (index, pseudonym) in pseudonyms.iter().enumerate() {
    if pseudonym.destination_namespace == in_namespace
    && pseudonym.kind == kind
    && &pseudonym.new_name == identifier {
      return Some(pseudonyms.remove(index))
    }
  }

  None
}



fn resolve_pseudonym (analyzer: &mut Analyzer, pseudonyms: &mut Vec<Pseudonym>, pseudonym: Pseudonym) -> Option<ContextKey> {
  match &pseudonym.payload {
    PseudonymPayload::Path(_) => resolve_pseudonym_path(analyzer, pseudonyms, pseudonym),
    PseudonymPayload::TypeExpression(_) => resolve_pseudonym_texpr(analyzer, pseudonyms, pseudonym),
  }
}


fn resolve_path (
  analyzer: &mut Analyzer,
  pseudonyms: &mut Vec<Pseudonym>, 
  relative_to: ContextKey,
  path: &Path,
  origin: SourceRegion,
) -> Option<ContextKey> {
  let mut base_name = Identifier::default();
              
  let mut resolved_key = relative_to;
  
  for ident in path.chain.iter() {
    let base = analyzer.context.items.get(resolved_key).expect("Internal error, invalid lowered key during pseudonym resolution");

    if let ContextItem::Namespace(namespace) = base {
      base_name.set(&namespace.canonical_name);

      resolved_key = if !path.absolute && resolved_key == relative_to {
        if let Some(local) = namespace.local_bindings.get_entry(ident) {
          local
        } else if let Some(pseudonym) = try_get_pseudonym(pseudonyms, resolved_key, PseudonymKind::Alias, ident) {
          // if this fails there has already been an error message and we can just bail
          // TODO should unresolved pseudonyms link an error item? (probably)
          resolve_pseudonym(analyzer, pseudonyms, pseudonym)?
        } else if let Some(core) = analyzer.context.core_bs.get_entry(ident) {
          core
        } else {
          analyzer.error(origin, format!("Namespace `{}` does not have access to an item named `{}`", base_name, ident));
          return None
        }
      } else if let Some(exported_key) = namespace.export_bindings.get_entry(ident) {
        exported_key
      } else if let Some(pseudonym) = try_get_pseudonym(pseudonyms, resolved_key, PseudonymKind::Export, ident) {
        // if this fails there has already been an error message and we can just bail
        // TODO should unresolved pseudonyms link an error item? (probably)
        resolve_pseudonym(analyzer, pseudonyms, pseudonym)?
      } else {
        analyzer.error(origin, format!("Namespace `{}` does not export an item named `{}`", base_name, ident));
        return None
      };
    } else {
      analyzer.error(origin, format!("{} is not a Namespace and has no exports", ident));
      return None
    }
  }

  Some(resolved_key)
}


fn resolve_texpr (analyzer: &mut Analyzer, pseudonyms: &mut Vec<Pseudonym>, relative_to: ContextKey, texpr: &TypeExpression) -> Option<ContextKey> {
  match &texpr.data {
    TypeExpressionData::Identifier(ident) => resolve_path(analyzer, pseudonyms, relative_to, &Path::from(ident.clone()), texpr.origin),
    TypeExpressionData::Path(path) => resolve_path(analyzer, pseudonyms, relative_to, &path, texpr.origin),
    TypeExpressionData::Pointer(box sub_texpr) => {
      let sub_key = resolve_texpr(analyzer, pseudonyms, relative_to, sub_texpr)?;

      Some(ty_from_anon_data(analyzer, TypeData::Pointer(sub_key), texpr.origin))
    },
    TypeExpressionData::Function { parameter_types, return_type } => {
      let mut param_keys = Vec::new();

      for param_texpr in parameter_types.iter() {
        param_keys.push(resolve_texpr(analyzer, pseudonyms, relative_to, param_texpr)?);
      }

      let ret_key = if let box Some(ret_texpr) = return_type { Some(resolve_texpr(analyzer, pseudonyms, relative_to, ret_texpr)?) } else { None };

      Some(ty_from_anon_data(analyzer, TypeData::Function { parameter_types: param_keys, return_type: ret_key }, texpr.origin))
    }
  }
}


fn register_resolved_pseudonym (analyzer: &mut Analyzer, pseudonym: Pseudonym, resolved_key: ContextKey) {
  let dest_ns =
    analyzer.context.items
      .get(pseudonym.destination_namespace)
      .expect("Internal error, pseudonym has invalid destination namespace key")
      .ref_namespace()
      .expect("Internal error, pseudonym destination key does not resolve to a namespace");

  match pseudonym.kind {
    PseudonymKind::Alias => {
      if let Some(existing_key) = dest_ns.local_bindings.get_entry(&pseudonym.new_name) {
        let existing_origin = 
          dest_ns.local_bindings
            .get_bind_location(existing_key)
            .expect("Internal error, local item has no binding source location");

        analyzer.error(pseudonym.origin, format!(
          "Namespace alias `{}` shadows an existing item, defined at [{}]",
          pseudonym.new_name,
          existing_origin,
        ))
      }

      unsafe { analyzer.context.items.get_unchecked_mut(pseudonym.destination_namespace).mut_namespace_unchecked() }
        .local_bindings.set_entry_bound(pseudonym.new_name, resolved_key, pseudonym.origin);
    },

    PseudonymKind::Export => {
      if let Some(existing_key) = dest_ns.export_bindings.get_entry(&pseudonym.new_name) {
        let existing_origin = 
          dest_ns.export_bindings
            .get_bind_location(existing_key)
            .expect("Internal error, export item has no binding source location");

        analyzer.error(pseudonym.origin, format!(
          "Namespace export `{}` shadows an existing item, defined at [{}]",
          pseudonym.new_name,
          existing_origin,
        ))
      }

      unsafe { analyzer.context.items.get_unchecked_mut(pseudonym.destination_namespace).mut_namespace_unchecked() }
        .export_bindings.set_entry_bound(pseudonym.new_name, resolved_key, pseudonym.origin);
    },
  }
}


fn resolve_pseudonym_path (analyzer: &mut Analyzer, pseudonyms: &mut Vec<Pseudonym>, pseudonym: Pseudonym) -> Option<ContextKey> {
  let resolved_key = if let PseudonymPayload::Path(path) = &pseudonym.payload {
    resolve_path(analyzer, pseudonyms, pseudonym.relative_to, path, pseudonym.origin)?
  } else {
    unreachable!("Internal error, resolve_pseudonym_path called on Pseudonym with improper PseudonymPayload");
  };

  register_resolved_pseudonym(analyzer, pseudonym, resolved_key);

  Some(resolved_key)
}


fn resolve_pseudonym_texpr (analyzer: &mut Analyzer, pseudonyms: &mut Vec<Pseudonym>, pseudonym: Pseudonym) -> Option<ContextKey> {
  let resolved_key = if let PseudonymPayload::TypeExpression(texpr) = &pseudonym.payload {
    resolve_texpr(analyzer, pseudonyms, pseudonym.relative_to, texpr)?
  } else {
    unreachable!("Internal error, resolve_pseudonym_texpr called on Pseudonym wiht improper PseudonymPayload");
  };

  register_resolved_pseudonym(analyzer, pseudonym, resolved_key);

  Some(resolved_key)
}