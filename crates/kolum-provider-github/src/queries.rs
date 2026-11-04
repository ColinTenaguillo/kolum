//! The GraphQL this adapter sends, in one place.

/// A project by owner and number: a user's or an organisation's, so both
/// are asked for and the one that exists answers.
pub const PROJECT: &str = r#"query($owner: String!, $number: Int!) {
  user(login: $owner) { projectV2(number: $number) { ...Project } }
  organization(login: $owner) { projectV2(number: $number) { ...Project } }
}
fragment Project on ProjectV2 {
  id title number url
  fields(first: 50) { nodes {
    ... on ProjectV2SingleSelectField { id name options { id name } }
  } }
}"#;

const ITEM: &str = r#"fragment Item on ProjectV2Item {
  id updatedAt
  fieldValues(first: 30) { nodes {
    ... on ProjectV2ItemFieldSingleSelectValue { name optionId field { ... on ProjectV2FieldCommon { name } } }
    ... on ProjectV2ItemFieldNumberValue { number field { ... on ProjectV2FieldCommon { name } } }
    ... on ProjectV2ItemFieldIterationValue { title iterationId field { ... on ProjectV2FieldCommon { name } } }
  } }
  content {
    __typename
    ... on DraftIssue { id title body createdAt updatedAt creator { login }
      assignees(first: 10) { nodes { id login name avatarUrl } } }
    ... on Issue { id number title body url state createdAt updatedAt author { login }
      repository { name } assignees(first: 10) { nodes { id login name avatarUrl } }
      labels(first: 20) { nodes { name } } parent { number title repository { name } } }
    ... on PullRequest { id number title body url state createdAt updatedAt author { login }
      repository { name } assignees(first: 10) { nodes { id login name avatarUrl } }
      labels(first: 20) { nodes { name } } }
  }
}"#;

pub fn items() -> String {
    format!(
        "query($id: ID!, $after: String) {{ node(id: $id) {{ ... on ProjectV2 {{ items(first: 100, after: $after) {{
  pageInfo {{ hasNextPage endCursor }} nodes {{ ...Item }} }} }} }} }}\n{ITEM}"
    )
}

pub fn item() -> String {
    format!("query($id: ID!) {{ node(id: $id) {{ ...Item }} }}\n{ITEM}")
}

pub const COMMENTS: &str = r#"query($id: ID!) { node(id: $id) {
  ... on Issue { comments(first: 100) { nodes { id author { login } body createdAt } } }
  ... on PullRequest { comments(first: 100) { nodes { id author { login } body createdAt } } }
} }"#;

pub const SEARCH: &str = r#"query($q: String!, $after: String) {
  search(query: $q, type: ISSUE, first: 50, after: $after) {
    pageInfo { hasNextPage endCursor }
    nodes {
      __typename
      ... on Issue { id number title body url state createdAt updatedAt author { login }
        repository { name } assignees(first: 10) { nodes { id login name avatarUrl } }
        labels(first: 20) { nodes { name } } }
      ... on PullRequest { id number title body url state createdAt updatedAt author { login }
        repository { name } assignees(first: 10) { nodes { id login name avatarUrl } }
        labels(first: 20) { nodes { name } } }
    }
  }
}"#;

pub const VIEWER: &str = "query { viewer { id login name avatarUrl } }";

pub const USERS: &str = r#"query($q: String!) { search(query: $q, type: USER, first: 20) {
  nodes { ... on User { id login name avatarUrl } } } }"#;

pub const SET_STATUS: &str = r#"mutation($project: ID!, $item: ID!, $field: ID!, $option: String!) {
  updateProjectV2ItemFieldValue(input: { projectId: $project, itemId: $item, fieldId: $field,
    value: { singleSelectOptionId: $option } }) { projectV2Item { id } } }"#;

pub const ADD_ASSIGNEES: &str = r#"mutation($id: ID!, $users: [ID!]!) {
  addAssigneesToAssignable(input: { assignableId: $id, assigneeIds: $users }) { clientMutationId } }"#;

pub const REMOVE_ASSIGNEES: &str = r#"mutation($id: ID!, $users: [ID!]!) {
  removeAssigneesFromAssignable(input: { assignableId: $id, assigneeIds: $users }) { clientMutationId } }"#;

pub const ADD_COMMENT: &str = r#"mutation($id: ID!, $body: String!) {
  addComment(input: { subjectId: $id, body: $body }) {
    commentEdge { node { id author { login } body createdAt } } } }"#;

pub const UPDATE_ISSUE: &str = r#"mutation($id: ID!, $title: String, $body: String) {
  updateIssue(input: { id: $id, title: $title, body: $body }) { clientMutationId } }"#;

pub const UPDATE_PULL: &str = r#"mutation($id: ID!, $title: String, $body: String) {
  updatePullRequest(input: { pullRequestId: $id, title: $title, body: $body }) { clientMutationId } }"#;

pub const UPDATE_DRAFT: &str = r#"mutation($id: ID!, $title: String, $body: String) {
  updateProjectV2DraftIssue(input: { draftIssueId: $id, title: $title, body: $body }) { clientMutationId } }"#;

pub const ADD_DRAFT: &str = r#"mutation($project: ID!, $title: String!, $body: String) {
  addProjectV2DraftIssue(input: { projectId: $project, title: $title, body: $body }) { projectItem { id } } }"#;

/// `afterId` null puts the item first.
pub const MOVE_ITEM: &str = r#"mutation($project: ID!, $item: ID!, $after: ID) {
  updateProjectV2ItemPosition(input: { projectId: $project, itemId: $item, afterId: $after }) { clientMutationId } }"#;
