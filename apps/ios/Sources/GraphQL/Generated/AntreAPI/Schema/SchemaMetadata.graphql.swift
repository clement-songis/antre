// @generated
// This file was automatically generated and should not be edited.

import ApolloAPI

nonisolated protocol AntreAPI_SelectionSet: ApolloAPI.SelectionSet & ApolloAPI.RootSelectionSet
where Schema == AntreAPI.SchemaMetadata {}

nonisolated protocol AntreAPI_InlineFragment: ApolloAPI.SelectionSet & ApolloAPI.InlineFragment
where Schema == AntreAPI.SchemaMetadata {}

nonisolated protocol AntreAPI_MutableSelectionSet: ApolloAPI.MutableRootSelectionSet
where Schema == AntreAPI.SchemaMetadata {}

nonisolated protocol AntreAPI_MutableInlineFragment: ApolloAPI.MutableSelectionSet & ApolloAPI.InlineFragment
where Schema == AntreAPI.SchemaMetadata {}

extension AntreAPI {
  typealias SelectionSet = AntreAPI_SelectionSet

  typealias InlineFragment = AntreAPI_InlineFragment

  typealias MutableSelectionSet = AntreAPI_MutableSelectionSet

  typealias MutableInlineFragment = AntreAPI_MutableInlineFragment

  nonisolated enum SchemaMetadata: ApolloAPI.SchemaMetadata {
    static let configuration: any ApolloAPI.SchemaConfiguration.Type = SchemaConfiguration.self

    private static let objectTypeMap: [String: ApolloAPI.Object] = [
      "AgentMutation": AntreAPI.Objects.AgentMutation,
      "BoolResult": AntreAPI.Objects.BoolResult,
      "ModelInfo": AntreAPI.Objects.ModelInfo,
      "ModelQuery": AntreAPI.Objects.ModelQuery,
      "MutationRoot": AntreAPI.Objects.MutationRoot,
      "QueryRoot": AntreAPI.Objects.QueryRoot,
      "SessionEntry": AntreAPI.Objects.SessionEntry,
      "SessionQuery": AntreAPI.Objects.SessionQuery,
      "StatusInfo": AntreAPI.Objects.StatusInfo
    ]

    static func objectType(forTypename typename: String) -> ApolloAPI.Object? {
      objectTypeMap[typename]
    }
  }

  nonisolated enum Objects {}
  nonisolated enum Interfaces {}
  nonisolated enum Unions {}

}